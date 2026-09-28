use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

const PORT_RANGE_START: u16 = 9222;
const PORT_RANGE_SIZE: u16 = 78; // 9222..=9299

/// Résolution par défaut du navigateur (fenêtre Chrome ET viewport CDP) :
/// 16:9 dense, plus confortable pour l'inspection visuelle et les
/// screenshots qu'un ratio non standard. Le défaut natif de chromiumoxide
/// (Viewport { width: 800, height: 600 }, ratio 4:3) produisait un rendu
/// exigu et un ratio incohérent avec la majorité des apps web modernes
/// pensées pour du 16:9/16:10 — corrigé ici plutôt que laissé au hasard du
/// défaut de la bibliothèque CDP. Configurable via SPECTRA_WINDOW_SIZE
/// ("1920x1080" ou "1600x900"), au format "<largeur>x<hauteur>".
const DEFAULT_WINDOW_WIDTH: u32 = 1920;
const DEFAULT_WINDOW_HEIGHT: u32 = 1080;

/// Parse `SPECTRA_WINDOW_SIZE` (ex: "1600x900") si présent et valide, sinon
/// retombe sur le défaut 1920x1080. Toute valeur malformée (pas de "x",
/// composants non numériques, zéro) est silencieusement ignorée au profit du
/// défaut plutôt que de faire échouer tout le lancement de Chrome pour une
/// variable d'environnement mal formée.
pub fn window_size() -> (u32, u32) {
    std::env::var("SPECTRA_WINDOW_SIZE")
        .ok()
        .and_then(|raw| {
            let (w, h) = raw.split_once(['x', 'X'])?;
            let w: u32 = w.trim().parse().ok()?;
            let h: u32 = h.trim().parse().ok()?;
            (w > 0 && h > 0).then_some((w, h))
        })
        .unwrap_or((DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// Tente d'abord un attach sur une instance déjà lancée pour ce projet,
    /// sinon spawn une nouvelle instance.
    Auto,
    /// Force le spawn d'une nouvelle instance (échoue si le port est déjà pris par autre chose).
    Spawn,
    /// Force l'attach (échoue si rien n'écoute sur le port dérivé).
    Attach,
}

pub struct LaunchOutcome {
    pub ws_url: String,
    pub port: u16,
    pub profile_dir: PathBuf,
    pub attached_existing: bool,
    /// Présent uniquement si cet appel a spawn un nouveau process Chrome
    /// (None si attach sur une instance existante). Le propriétaire de ce
    /// champ est responsable de garder le process vivant tant que la session
    /// est utilisée, et de le tuer proprement à la fermeture (voir session.rs).
    pub spawned_child: Option<chromiumoxide::browser::Browser>,
}

/// Dérive un port stable dans [9222, 9299] à partir du chemin absolu du projet,
/// pour permettre plusieurs projets actifs simultanément sans collision.
pub fn derive_port(project_key: &str) -> u16 {
    let mut hasher = Sha256::new();
    hasher.update(project_key.as_bytes());
    let digest = hasher.finalize();
    let n = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    PORT_RANGE_START + (n % PORT_RANGE_SIZE as u32) as u16
}

pub fn spectra_home() -> PathBuf {
    let base = dirs_local_appdata();
    base.join("Spectra")
}

fn dirs_local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn profile_dir_for(project_key: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(project_key.as_bytes());
    let digest = hasher.finalize();
    let short_hash = hex_prefix(&digest, 12);
    spectra_home().join("profiles").join(short_hash)
}

fn hex_prefix(bytes: &[u8], nchars: usize) -> String {
    let mut s = String::new();
    for b in bytes {
        if s.len() >= nchars {
            break;
        }
        s.push_str(&format!("{:02x}", b));
    }
    s.truncate(nchars);
    s
}

async fn probe_existing(port: u16) -> Option<String> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let client = reqwest_or_none()?;
    let resp = client.get(&url).timeout(Duration::from_millis(600)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("webSocketDebuggerUrl")?.as_str().map(|s| s.to_string())
}

fn reqwest_or_none() -> Option<reqwest::Client> {
    Some(reqwest::Client::new())
}

/// Lance ou rattache Chrome pour un projet donné.
/// `project_key` est typiquement le chemin absolu canonique du dossier projet.
pub async fn launch_or_attach(
    project_key: &str,
    mode: LaunchMode,
    headless: bool,
    chrome_binary: Option<&Path>,
) -> Result<LaunchOutcome> {
    let port = derive_port(project_key);
    let profile_dir = profile_dir_for(project_key);

    if mode != LaunchMode::Spawn {
        if let Some(ws_url) = probe_existing(port).await {
            return Ok(LaunchOutcome {
                ws_url,
                port,
                profile_dir,
                attached_existing: true,
                spawned_child: None,
            });
        }
        if mode == LaunchMode::Attach {
            bail!("aucune instance Chrome trouvée sur le port {port} pour ce projet (mode attach strict)");
        }
    }

    std::fs::create_dir_all(&profile_dir)
        .with_context(|| format!("création du profil {}", profile_dir.display()))?;

    acquire_lock(&profile_dir, port)?;

    let (browser, ws_url) = spawn_chrome(port, &profile_dir, headless, chrome_binary).await?;
    Ok(LaunchOutcome {
        ws_url,
        port,
        profile_dir,
        attached_existing: false,
        spawned_child: Some(browser),
    })
}

fn acquire_lock(profile_dir: &Path, port: u16) -> Result<()> {
    let lock_path = profile_dir.join(".lock");
    if lock_path.exists() {
        if let Ok(existing_pid) = std::fs::read_to_string(&lock_path) {
            if process_alive(existing_pid.trim()) {
                bail!(
                    "un autre spectra-server (pid {}) gère déjà le port {} pour ce profil — attache-toi plutôt qu'un nouveau spawn",
                    existing_pid.trim(),
                    port
                );
            }
        }
    }
    std::fs::write(&lock_path, std::process::id().to_string())?;
    Ok(())
}

fn process_alive(pid: &str) -> bool {
    let Ok(pid_n) = pid.parse::<u32>() else { return false };
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid_n}"), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid_n.to_string()))
        .unwrap_or(false)
}

/// Emplacements standards où chercher un exécutable Chrome/Chromium sur
/// Windows, utilisés pour un diagnostic précoce (voir `spawn_chrome`) — ce
/// n'est pas exhaustif (chromiumoxide fait sa propre détection en interne),
/// juste de quoi produire un message d'erreur actionnable si aucun n'existe.
#[cfg(windows)]
const CHROME_CANDIDATE_PATHS: &[&str] = &[
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files\Chromium\Application\chrome.exe",
];

#[cfg(windows)]
fn any_chrome_found(explicit: Option<&Path>) -> bool {
    if let Some(p) = explicit {
        return p.exists();
    }
    if let Ok(custom) = std::env::var("SPECTRA_CHROME_PATH") {
        if Path::new(&custom).exists() {
            return true;
        }
    }
    CHROME_CANDIDATE_PATHS.iter().any(|p| Path::new(p).exists())
}

#[cfg(not(windows))]
fn any_chrome_found(_explicit: Option<&Path>) -> bool {
    // Sur macOS/Linux, on laisse chromiumoxide faire sa détection standard
    // (PATH, emplacements usuels) sans dupliquer sa logique ici — ce garde-fou
    // reste spécifique à Windows où le projet a été développé et testé.
    true
}

async fn spawn_chrome(
    port: u16,
    profile_dir: &Path,
    headless: bool,
    chrome_binary: Option<&Path>,
) -> Result<(chromiumoxide::browser::Browser, String)> {
    use chromiumoxide::browser::{Browser, BrowserConfig};

    if !any_chrome_found(chrome_binary) {
        bail!(
            "Chrome introuvable. Spectra pilote Chrome via CDP et a besoin d'une installation \
             existante — installez Google Chrome (https://www.google.com/chrome/) ou définissez \
             la variable d'environnement SPECTRA_CHROME_PATH vers votre binaire Chrome/Chromium \
             (ex: SPECTRA_CHROME_PATH=C:\\chemin\\vers\\chrome.exe)."
        );
    }

    let stealth = std::env::var("SPECTRA_STEALTH").as_deref() == Ok("1");

    let (window_w, window_h) = window_size();
    let mut args = vec![
        format!("--remote-debugging-port={port}"),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        format!("--window-size={window_w},{window_h}"),
        // Force aussi la position pour éviter qu'un Chrome "avec tête" ne
        // s'ouvre partiellement hors écran sur un second moniteur déconnecté.
        "--window-position=0,0".to_string(),
    ];
    if stealth {
        // Mode discret : retire le flag CDP le plus visible côté page
        // (`navigator.webdriver === true`, détectable en une ligne de JS par
        // n'importe quel site). `--disable-blink-features=AutomationControlled`
        // seul ne suffit pas si `--enable-automation` (présent dans les
        // DEFAULT_ARGS de chromiumoxide) reste dans la commandline — certains
        // sites vérifient aussi la bannière "Chrome est contrôlé par un
        // logiciel de test automatisé" que ce flag fait disparaître. D'où
        // `disable_default_args()` plus bas : on réintègre nous-mêmes les 24
        // flags utiles des DEFAULT_ARGS de chromiumoxide, en excluant
        // uniquement `--enable-automation`.
        args.push("--disable-blink-features=AutomationControlled".to_string());
        // User-agent custom optionnel : chromiumoxide n'expose pas de méthode
        // builder dédiée (seulement une lecture async post-connexion), donc
        // passage par flag CLI standard --user-agent, reconnu nativement par
        // Chrome. Un user-agent par défaut de test automatisé (souvent
        // `HeadlessChrome/...`, cf. V26 de COMPARISON.md) est un signal de
        // détection ; SPECTRA_USER_AGENT permet de le remplacer par un UA de
        // navigateur standard si le site cible le vérifie.
        if let Ok(ua) = std::env::var("SPECTRA_USER_AGENT") {
            args.push(format!("--user-agent={ua}"));
        }
    }
    // SPECTRA_MINIMAL_CHROME=0 pour désactiver ces flags additionnels lors
    // d'une mesure A/B comparative — voir COMPARISON.md pour la méthodologie
    // et les chiffres. Activé par défaut (bascule volontaire, pas juste un
    // outil de debug oublié). Désactivé aussi en mode stealth : un GPU
    // désactivé change le rendu WebGL de façon détectable (getParameter
    // renvoie un renderer logiciel, pas une vraie carte graphique) — un vrai
    // navigateur humain garde son GPU actif, donc le stealth n'a pas intérêt
    // à ressembler à un Chrome "minimal" optimisé pour un pilotage programmatique.
    if std::env::var("SPECTRA_MINIMAL_CHROME").as_deref() != Ok("0") && !stealth {
        // Flags additionnels au-delà des DEFAULT_ARGS de chromiumoxide (déjà
        // 25, voir browser.rs de la lib) — retirent des sous-systèmes dont un
        // agent IA pilotant Chrome n'a jamais l'usage : rendu GPU (inutile en
        // headless, où il n'y a de toute façon aucun affichage physique),
        // notifications système, requêtes de fiabilité réseau en
        // arrière-plan, cache de navigation arrière/avant (jamais utilisé par
        // un pilotage programmatique).
        args.extend([
            "--disable-gpu".to_string(),
            "--disable-software-rasterizer".to_string(),
            "--disable-notifications".to_string(),
            "--disable-domain-reliability".to_string(),
            "--disable-features=Translate,BackForwardCache,AcceptCHFrame,MediaRouter,OptimizationHints,PrivacySandboxSettings4".to_string(),
            "--no-pings".to_string(),
            "--disable-background-networking".to_string(),
        ]);
    }
    if stealth {
        // Réintègre manuellement les DEFAULT_ARGS utiles de chromiumoxide
        // (stabilité/perf de base), à l'exception de `--enable-automation`
        // (le flag qui déclenche la bannière "Chrome est contrôlé" et
        // `navigator.webdriver`). `disable_default_args()` plus bas retire
        // TOUS les DEFAULT_ARGS d'un coup — sans ce ré-ajout manuel, on
        // perdrait aussi des flags de stabilité sans rapport avec la
        // détection (ex: --disable-dev-shm-usage, qui évite des crashs sur
        // certains environnements à /dev/shm limité).
        args.extend([
            "--disable-background-networking".to_string(),
            "--enable-features=NetworkService,NetworkServiceInProcess".to_string(),
            "--disable-background-timer-throttling".to_string(),
            "--disable-backgrounding-occluded-windows".to_string(),
            "--disable-breakpad".to_string(),
            "--disable-client-side-phishing-detection".to_string(),
            "--disable-component-extensions-with-background-pages".to_string(),
            "--disable-default-apps".to_string(),
            "--disable-dev-shm-usage".to_string(),
            "--disable-extensions".to_string(),
            "--disable-features=TranslateUI".to_string(),
            "--disable-hang-monitor".to_string(),
            "--disable-ipc-flooding-protection".to_string(),
            "--disable-popup-blocking".to_string(),
            "--disable-prompt-on-repost".to_string(),
            "--disable-renderer-backgrounding".to_string(),
            "--disable-sync".to_string(),
            "--force-color-profile=srgb".to_string(),
            "--metrics-recording-only".to_string(),
            "--password-store=basic".to_string(),
            "--use-mock-keychain".to_string(),
            "--enable-blink-features=IdleDetection".to_string(),
            "--lang=fr-FR".to_string(),
        ]);
    }
    let mut builder = BrowserConfig::builder().user_data_dir(profile_dir).port(port).args(args);
    if stealth {
        builder = builder.disable_default_args();
    }
    let mut builder = builder
        // Fixe la taille de la fenêtre Chrome ET le viewport CDP
        // (Emulation.setDeviceMetricsOverride) à la même résolution. Les deux
        // sont nécessaires : --window-size ne dimensionne que la fenêtre du
        // process, alors que le rendu réel (screenshots, layout, snapshot
        // d'accessibilité) suit le viewport CDP — qui, sans réglage explicite,
        // reste au défaut 800x600 de chromiumoxide même quand la fenêtre est
        // plus grande, y compris en headless où il n'y a aucune fenêtre
        // physique pour le "deviner".
        .window_size(window_w, window_h)
        .viewport(chromiumoxide::handler::viewport::Viewport {
            width: window_w,
            height: window_h,
            ..Default::default()
        });

    if !headless {
        builder = builder.with_head();
    }
    let effective_chrome_path = chrome_binary
        .map(|p| p.to_path_buf())
        .or_else(|| std::env::var("SPECTRA_CHROME_PATH").ok().map(PathBuf::from));
    if let Some(bin) = &effective_chrome_path {
        builder = builder.chrome_executable(bin);
    }

    let config = builder.build().map_err(|e| anyhow::anyhow!("config chromiumoxide invalide: {e}"))?;
    let (browser, handler) = Browser::launch(config).await.with_context(|| {
        format!(
            "échec du démarrage de Chrome sur le port {port}. Causes possibles : \
             port déjà utilisé par un autre process (relancez ou changez de projet), \
             profil verrouillé par une instance Chrome zombie (voir scripts/kill-all-spectra.ps1 \
             pour nettoyer), ou permissions insuffisantes sur {}.",
            profile_dir.display()
        )
    })?;
    let ws_url = browser.websocket_address().to_string();

    // Le handler pompe les évènements CDP (navigation, réseau, console) en tâche
    // de fond pendant toute la vie du Browser — spectra-session le relaie ensuite
    // vers les buffers console/network exposés par les tools.
    tokio::spawn(async move {
        use futures::StreamExt;
        let mut handler = handler;
        while handler.next().await.is_some() {}
    });

    Ok((browser, ws_url))
}
