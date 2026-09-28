use anyhow::{bail, Context, Result};
use spectra_cdp::BrowserSession;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// Source du runtime Node embarquée directement dans le binaire à la
/// compilation (`include_str!`) — le fichier `.mjs` n'est donc jamais une
/// dépendance externe séparée qui pourrait manquer ou se désynchroniser du
/// binaire : `browser_run_recipe` a échoué silencieusement à tout coup avant
/// ce fix, faute d'un vrai mécanisme d'installation (aucun script ne
/// copiait jamais `recipe-runner.mjs` vers son emplacement attendu).
const RECIPE_RUNNER_SOURCE: &str = include_str!("../../../runtime/recipe-runner.mjs");

/// Localise le runtime Node, en l'écrivant sur disque à la demande depuis la
/// source embarquée (`RECIPE_RUNNER_SOURCE`) si absent ou différent du
/// contenu attendu — auto-installation transparente, jamais besoin d'un
/// script d'installation séparé à lancer manuellement.
fn runner_path() -> Result<PathBuf> {
    let path = spectra_cdp::launcher::spectra_home().join("runtime").join("recipe-runner.mjs");
    let needs_write = match std::fs::read_to_string(&path) {
        Ok(existing) => existing != RECIPE_RUNNER_SOURCE,
        Err(_) => true,
    };
    if needs_write {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("création de {}", parent.display()))?;
        }
        std::fs::write(&path, RECIPE_RUNNER_SOURCE)
            .with_context(|| format!("écriture de {}", path.display()))?;
    }
    Ok(path)
}

/// Localise le dossier recipes du projet en résolvant `<project>` soit comme
/// chemin absolu, soit comme nom cherché depuis le cwd du serveur.
fn recipes_dir(project: &str) -> PathBuf {
    let candidate = PathBuf::from(project);
    let base = if candidate.is_absolute() { candidate } else { PathBuf::from(".").join(project) };
    base.join(".spectra").join("recipes")
}

pub async fn run(
    session: &BrowserSession,
    project: &str,
    recipe: &str,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let recipe_path = recipes_dir(project).join(format!("{recipe}.mjs"));
    if !recipe_path.exists() {
        bail!(
            "recipe introuvable: {} — vérifie que .spectra/recipes/{}.mjs existe dans le projet",
            recipe_path.display(),
            recipe
        );
    }

    let node = which_node().context("node introuvable dans le PATH — installe Node.js pour utiliser browser_run_recipe (les autres tools natifs restent utilisables sans Node)")?;
    check_node_version(&node).await?;

    let ws_url = session.ws_url().to_string();
    let project_dir = recipes_dir(project)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| project.to_string());

    let stdin_payload = serde_json::json!({
        "cdp_ws_url": ws_url,
        "project_dir": project_dir,
        "recipe_path": recipe_path.to_string_lossy(),
        "args": args.unwrap_or(serde_json::Value::Null),
    });

    let mut child = Command::new(node)
        .arg(runner_path()?)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("lancement de recipe-runner.mjs")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(stdin_payload.to_string().as_bytes()).await?;
        stdin.shutdown().await?;
    }

    // `wait_with_output()` consomme `self` (Child) — si le timeout expire
    // pendant cet await, on perd tout handle sur le process et ne peut plus
    // le tuer explicitement. `tokio::process::Child` n'implémente pas
    // `kill_on_drop` par défaut (comportement Tokio standard), donc une
    // recipe qui ne se termine jamais (boucle infinie, attente sur un
    // élément qui n'apparaît jamais) laissait le process Node tourner
    // indéfiniment après que l'agent ait déjà reçu son erreur de timeout —
    // confirmé en test isolé (recipe `await new Promise(() => {})`, jamais
    // résolue : le process Node restait vivant plusieurs minutes après le
    // timeout Rust, sans intervention manuelle il ne serait jamais mort).
    // Corrigé en lisant stdout/stderr séparément (sans consommer `child`),
    // pour garder la capacité de `child.kill()` si le timeout se déclenche.
    let mut stdout_pipe = child.stdout.take().context("stdout du recipe-runner indisponible")?;
    let mut stderr_pipe = child.stderr.take().context("stderr du recipe-runner indisponible")?;

    let read_fut = async {
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();
        let (r1, r2) = tokio::join!(
            stdout_pipe.read_to_end(&mut stdout_buf),
            stderr_pipe.read_to_end(&mut stderr_buf)
        );
        r1?;
        r2?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, stdout_buf, stderr_buf))
    };

    let (status, stdout_buf, stderr_buf) = match tokio::time::timeout(Duration::from_secs(30), read_fut).await {
        Ok(result) => result?,
        Err(_) => {
            // Timeout dépassé: le process n'a jamais rendu la main. On le
            // tue explicitement plutôt que de le laisser en arrière-plan —
            // `child` est toujours à nous à ce stade puisqu'on ne l'a jamais
            // passé à `wait_with_output()`.
            child.kill().await.ok();
            bail!("timeout de 30s dépassé pour la recipe '{recipe}' — le process a été arrêté");
        }
    };

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_buf);
        bail!("recipe '{recipe}' terminée en erreur: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&stdout_buf);
    serde_json::from_str(&stdout).context("sortie de la recipe non-JSON")
}

/// `recipe-runner.mjs` utilise le `WebSocket` global natif de Node, stable
/// seulement depuis Node 22 (expérimental avant, absent avant Node 20). Sans
/// cette vérification préalable, un utilisateur sur une version plus
/// ancienne obtiendrait un message d'erreur Node opaque
/// (`ReferenceError: WebSocket is not defined`, sans plus de contexte) au
/// lieu d'un message actionnable pointant vers la vraie cause — même
/// discipline déjà appliquée ailleurs dans le projet (messages d'erreur
/// actionnables au démarrage de Chrome, V3 ; erreurs JS nettoyées sur
/// `browser_evaluate`, V15).
async fn check_node_version(node: &Path) -> Result<()> {
    let output = tokio::process::Command::new(node)
        .arg("--version")
        .output()
        .await
        .context("impossible d'exécuter node --version")?;
    let version_str = String::from_utf8_lossy(&output.stdout);
    let major: Option<u32> = version_str.trim().trim_start_matches('v').split('.').next().and_then(|s| s.parse().ok());
    match major {
        Some(m) if m < 22 => bail!(
            "Node.js {} détecté, mais browser_run_recipe nécessite Node.js 22 ou plus récent \
             (le runtime utilise le WebSocket global natif, stable seulement depuis cette version). \
             Mets à jour Node.js pour utiliser les recipes — les autres tools natifs restent utilisables sans changement.",
            version_str.trim()
        ),
        _ => Ok(()),
    }
}

fn which_node() -> Option<PathBuf> {
    let candidates = if cfg!(windows) { vec!["node.exe", "node"] } else { vec!["node"] };
    for c in candidates {
        if let Ok(path) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path) {
                let full = dir.join(c);
                if full.exists() {
                    return Some(full);
                }
            }
        }
    }
    None
}
