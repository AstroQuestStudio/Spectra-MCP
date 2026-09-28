use anyhow::{Context, Result};
use chromiumoxide::cdp::browser_protocol::accessibility::EnableParams;
use chromiumoxide::page::Page;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::OnceLock;
use tokio::sync::Mutex as AsyncMutex;

use crate::raw_ax::{parse_nodes, RawAxNode, RawGetFullAxTree};
use crate::refs::make_ref;

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotNode {
    pub r#ref: String,
    pub role: String,
    pub name: String,
    pub depth: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub url: String,
    pub text: String,
    pub ref_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a11y_issues: Option<Vec<A11yIssue>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct A11yIssue {
    pub r#ref: Option<String>,
    pub rule: String,
    pub message: String,
}

/// Rôles considérés interactifs pour la règle "nom accessible manquant" —
/// même liste que celle utilisée pour décider si un noeud mérite une ref
/// dans le rendu texte (voir `is_interactive_role`), réutilisée ici pour
/// rester cohérente : un élément qu'on laisserait cliquable sans ref serait
/// de toute façon un problème d'accessibilité plus grave encore.
const A11Y_INTERACTIVE_ROLES: &[&str] =
    &["button", "link", "textbox", "checkbox", "radio", "combobox", "searchbox"];

/// Détection WCAG volontairement basique, en post-traitement pur de l'arbre
/// déjà collecté pour le snapshot (aucun appel CDP supplémentaire, donc
/// aucun risque de latence ou de blocage additionnel). Ce n'est PAS un
/// remplacement d'un vrai audit (axe-core, Lighthouse) : ça couvre 2 règles
/// à fort signal et faible taux de faux positifs, pensées comme un premier
/// avertissement pour un agent IA, pas un rapport de conformité.
///
/// Une troisième règle ("image sans texte alternatif") a été implémentée,
/// testée en conditions réelles, puis RETIRÉE : deux itérations de filtrage
/// (parent direct nommé, puis remontée récursive au travers des wrappers de
/// présentation) n'ont pas suffi à éliminer un faux-positif massif (70+
/// occurrences identiques sur une seule page) causé par un motif structurel
/// non identifié dans le temps disponible. Plutôt que de livrer une règle
/// dont le signal est noyé dans le bruit, elle a été retirée — à reprendre
/// en V4 avec une investigation plus poussée (probablement lier l'image à
/// son `backend_dom_node_id` et inspecter le DOM réel plutôt que de remonter
/// l'arbre AX seul).
fn detect_a11y_issues(
    nodes: &[RawAxNode],
    by_id: &HashMap<String, &RawAxNode>,
    ref_by_node_id: &HashMap<String, String>,
) -> Vec<A11yIssue> {
    let mut issues = Vec::new();

    for node in nodes {
        if node.ignored {
            continue;
        }

        // Règle 1: élément interactif sans nom accessible — un bouton ou lien
        // sans texte ni aria-label est inutilisable au clavier/lecteur d'écran,
        // et souvent aussi un piège pour un agent IA qui ne peut pas savoir ce
        // que l'élément fait.
        if A11Y_INTERACTIVE_ROLES.contains(&node.role.as_str()) && node.name.trim().is_empty() {
            issues.push(A11yIssue {
                r#ref: ref_by_node_id.get(&node.node_id).cloned(),
                rule: "missing-accessible-name".to_string(),
                message: format!("Élément \"{}\" interactif sans nom accessible (texte, aria-label, ou alt manquant)", node.role),
            });
        }

        // Règle 3: rôle interactif imbriqué dans un autre rôle interactif —
        // structurellement incohérent en ARIA (ex: un bouton dans un lien) et
        // source de comportement ambigu au clavier/lecteur d'écran.
        if A11Y_INTERACTIVE_ROLES.contains(&node.role.as_str()) {
            if let Some(parent_id) = &node.parent_id {
                if let Some(parent) = by_id.get(parent_id) {
                    if A11Y_INTERACTIVE_ROLES.contains(&parent.role.as_str()) {
                        issues.push(A11yIssue {
                            r#ref: ref_by_node_id.get(&node.node_id).cloned(),
                            rule: "nested-interactive".to_string(),
                            message: format!(
                                "Élément \"{}\" imbriqué dans un élément \"{}\" — structure ARIA ambiguë",
                                node.role, parent.role
                            ),
                        });
                    }
                }
            }
        }
    }

    issues
}

/// Rôles purement présentationnels/structurels que l'on élague du rendu texte:
/// ils n'apportent rien à un agent qui doit cliquer/lire, et gonflent le
/// snapshot de 30-50% sur des pages React/MUI/Tailwind typiques (wrappers
/// de div génériques sans rôle sémantique).
const NOISE_ROLES: &[&str] = &["generic", "none", "presentation", "InlineTextBox"];

/// CDP exige `Accessibility.enable` avant tout `getFullAXTree` (sinon Chrome
/// répond une erreur laconique). On l'active une seule fois par page
/// (target_id), pas à chaque snapshot.
static ENABLED_PAGES: OnceLock<AsyncMutex<HashSet<String>>> = OnceLock::new();

/// Un noeud tel que rendu dans le texte du snapshot, gardé en mémoire entre
/// deux captures pour permettre un diff structurel (voir `diff_structural`).
/// `backend_id` est la clé stable de comparaison : Chrome garde le même
/// `backendDOMNodeId` pour un élément DOM tant qu'il n'est pas détruit et
/// recréé (contrairement aux refs `[eN]`, qui sont un simple index de
/// parcours recalculé à chaque capture et se décalent dès qu'un noeud est
/// ajouté/retiré n'importe où avant elles dans l'arbre).
#[derive(Debug, Clone, PartialEq)]
struct RenderedNode {
    backend_id: i64,
    role: String,
    name: String,
    depth: u32,
    r#ref: Option<String>,
    state_flags: Vec<&'static str>,
}

/// Dernière capture structurée par page (target_id), pour le diff demandé
/// via `diff_only`.
///
/// V4 : remplace le diff textuel naïf ligne-à-ligne (V2/V3) par une
/// comparaison par `backend_id` plutôt que par contenu de ligne. La version
/// texte avait un vrai défaut silencieux : les refs `[eN]` étant recalculées
/// à chaque capture, un élément supprimé quelque part au milieu de l'arbre
/// décale toutes les refs qui le suivent, et une comparaison ligne-à-ligne
/// peut alors soit produire du bruit sur des lignes qui n'ont "que" changé de
/// ref sans changer de contenu, soit — plus grave, observé en test isolé
/// (page HTML minimale, un bouton qui retire un `<div>` avant 3 autres
/// boutons) — masquer complètement une vraie suppression si le texte des
/// lignes restantes se recombine par coïncidence. Un diff par `backend_id`
/// ne peut pas se tromper de cette façon : un noeud présent avant/absent
/// après est une suppression, quel que soit l'effet sur la numérotation des
/// refs voisines.
///
/// Reste volontairement une comparaison Rust pure entre deux captures déjà
/// obtenues via `getFullAXTree` (aucun nouvel abonnement CDP asynchrone) —
/// une tentative antérieure de détection événementielle via
/// `MutationObserver` injecté côté page s'était bloquée indéfiniment en test
/// réel, risque qu'on ne reproduit pas ici.
static LAST_SNAPSHOT_STRUCT: OnceLock<AsyncMutex<HashMap<String, Vec<RenderedNode>>>> = OnceLock::new();

fn last_snapshot_registry() -> &'static AsyncMutex<HashMap<String, Vec<RenderedNode>>> {
    LAST_SNAPSHOT_STRUCT.get_or_init(|| AsyncMutex::new(HashMap::new()))
}

async fn ensure_accessibility_enabled(page: &Page) -> Result<()> {
    let target = page.target_id().inner().to_string();
    let set = ENABLED_PAGES.get_or_init(|| AsyncMutex::new(HashSet::new()));
    let mut set = set.lock().await;
    if set.contains(&target) {
        return Ok(());
    }
    page.execute(EnableParams::default())
        .await
        .context("Accessibility.enable a échoué")?;
    set.insert(target);
    Ok(())
}

/// Capture et condense l'accessibility tree de la page en texte hiérarchique
/// avec refs courtes [eN]. Enregistre la table ref -> backend_node_id dans le
/// registre global (crate::refs) indexé par target_id: c'est ce qui invalide
/// automatiquement les refs du snapshot précédent dès qu'on en reprend un nouveau.
///
/// Utilise `RawGetFullAxTree` (JSON brut) plutôt que le type
/// `GetFullAxTreeParams`/`AxNode` de chromiumoxide_cdp 0.7.0 : ce dernier fait
/// échouer TOUTE désérialisation dès qu'un noeud a une `ignoredReason` hors de
/// sa couverture (ex: "uninteresting", "notRendered" — catégorie entière
/// manquante dans l'enum `AxPropertyName` généré). Voir raw_ax.rs.
pub async fn capture(page: &Page) -> Result<Snapshot> {
    capture_full(page, false, false, DetailLevel::Full).await
}

/// Comme `capture`, mais si `diff_only` est vrai et qu'un snapshot précédent
/// existe pour cette page, le texte retourné est un résumé compact du
/// changement plutôt que l'arbre complet — voir `LAST_SNAPSHOT_TEXT`.
pub async fn capture_with_diff(page: &Page, diff_only: bool) -> Result<Snapshot> {
    capture_full(page, diff_only, false, DetailLevel::Full).await
}

/// Trois niveaux de verbosité du texte retourné, pour laisser un agent
/// contrôler explicitement son propre coût en tokens plutôt que de toujours
/// recevoir l'arbre complet même quand il n'a besoin que des cibles
/// actionnables. N'affecte jamais `diff_only` (déjà minimal par nature) ni
/// `check_a11y` (analyse la même donnée sous-jacente quel que soit le niveau).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailLevel {
    /// Comportement historique : arbre complet, hiérarchie indentée, tous les
    /// noeuds structurels (`generic`/`heading`/`paragraph` sans nom propre
    /// inclus) en plus des éléments actionnables/nommés.
    Full,
    /// Ne garde que les noeuds qui ont reçu une ref `[eN]` (actionnables ou
    /// nommés) — retire les noeuds purement structurels (`main`, `banner`,
    /// `paragraph` vide de nom...) qui n'apportent rien à un agent qui doit
    /// cliquer/lire du contenu précis. Conserve l'indentation d'origine :
    /// la hiérarchie reste lisible même sans les noeuds intermédiaires.
    Compact,
    /// Une ligne par ref, sans indentation ni hiérarchie — le strict minimum
    /// pour un agent qui a juste besoin de savoir "quelles refs existent et
    /// que représentent-elles", typiquement après avoir déjà lu un snapshot
    /// complet une première fois sur cette page.
    RefsOnly,
}

impl DetailLevel {
    pub fn parse(s: Option<&str>) -> Self {
        match s {
            Some("compact") => DetailLevel::Compact,
            Some("refs_only") => DetailLevel::RefsOnly,
            _ => DetailLevel::Full,
        }
    }
}

/// Comme `capture`, avec en plus une passe de détection WCAG basique
/// (`a11y_issues` dans le retour) si `check_a11y` est vrai — voir
/// `detect_a11y_issues`. Aucun coût si `false` (défaut).
pub async fn capture_full(
    page: &Page,
    diff_only: bool,
    check_a11y: bool,
    detail: DetailLevel,
) -> Result<Snapshot> {
    capture_inner(page, diff_only, check_a11y, detail, None).await
}

/// Comme `capture_full`, mais si `root_backend_id` est fourni, ne rend que le
/// sous-arbre enraciné à ce nœud (résolu depuis une ref `[eN]` d'un snapshot
/// précédent, voir `resolve_ref` côté appelant) plutôt que la page entière.
///
/// Toujours un unique `Accessibility.getFullAXTree` (même coût CDP qu'un
/// snapshot complet — Chrome ne permet pas d'interroger un sous-arbre
/// directement), le filtrage est un post-traitement pur sur l'arbre déjà
/// récupéré. Le gain est côté texte retourné à l'agent (une modale dans une
/// page dense peut ne représenter que quelques % des nœuds totaux), pas côté
/// latence réseau/CDP. Le diff structurel (`diff_only`) continue de comparer
/// uniquement ce sous-arbre d'un appel à l'autre — donc un `root_ref` qui
/// change de backend_id (élément recréé) invalide silencieusement l'ancien
/// diff, exactement comme un changement de page le ferait pour un snapshot
/// plein cadre.
pub async fn capture_scoped(
    page: &Page,
    diff_only: bool,
    check_a11y: bool,
    detail: DetailLevel,
    root_backend_id: i64,
) -> Result<Snapshot> {
    capture_inner(page, diff_only, check_a11y, detail, Some(root_backend_id)).await
}

async fn capture_inner(
    page: &Page,
    diff_only: bool,
    check_a11y: bool,
    detail: DetailLevel,
    root_backend_id: Option<i64>,
) -> Result<Snapshot> {
    ensure_accessibility_enabled(page).await?;

    let raw = page
        .execute(RawGetFullAxTree::default())
        .await
        .context("Accessibility.getFullAXTree a échoué")?;
    let nodes = parse_nodes(&raw.result);

    let by_id: HashMap<String, &RawAxNode> =
        nodes.iter().map(|n| (n.node_id.clone(), n)).collect();

    let root = match root_backend_id {
        Some(bid) => nodes.iter().find(|n| n.backend_dom_node_id == Some(bid)),
        None => nodes.iter().find(|n| n.parent_id.is_none()).or_else(|| nodes.first()),
    };

    let scope_missing = root_backend_id.is_some() && root.is_none();

    let mut full_text = String::new();
    let mut ref_entries = Vec::new();
    let mut index = 0usize;
    let mut structured = Vec::new();

    if let Some(root) = root {
        render_node(root, &by_id, 0, &mut full_text, &mut ref_entries, &mut index, &mut structured);
    } else if scope_missing {
        full_text.push_str("(root_ref ne correspond plus à aucun nœud du DOM actuel — l'élément a peut-être disparu depuis le dernier snapshot)\n");
    }

    let url = page.url().await.ok().flatten().unwrap_or_default();
    let ref_count = ref_entries.len();
    let target = page.target_id().inner().to_string();

    let a11y_issues = if check_a11y {
        // backend_dom_node_id -> node_id, pour retrouver la ref (indexée par
        // node_id via ref_entries) à partir d'un noeud de l'arbre WCAG.
        let node_id_by_backend: HashMap<i64, &str> = nodes
            .iter()
            .filter_map(|n| n.backend_dom_node_id.map(|bid| (bid, n.node_id.as_str())))
            .collect();
        let ref_by_node_id: HashMap<String, String> = ref_entries
            .iter()
            .filter_map(|(r, backend_id)| {
                node_id_by_backend.get(backend_id).map(|nid| (nid.to_string(), r.clone()))
            })
            .collect();
        Some(detect_a11y_issues(&nodes, &by_id, &ref_by_node_id))
    } else {
        None
    };

    crate::refs::store(&target, ref_entries).await;

    let text = if diff_only {
        let mut registry = last_snapshot_registry().lock().await;
        let previous = registry.get(&target).cloned();
        let diff_text = match previous {
            Some(prev) => diff_structural(&prev, &structured),
            None => format!("(premier snapshot de cette page, pas de diff possible)\n\n{full_text}"),
        };
        registry.insert(target, structured);
        diff_text
    } else {
        let rendered = render_at_detail_level(&structured, &full_text, detail);
        let mut registry = last_snapshot_registry().lock().await;
        registry.insert(target, structured);
        rendered
    };

    Ok(Snapshot { url, text, ref_count, a11y_issues })
}

/// Reconstruit le texte à partir de `structured` (déjà collecté pendant le
/// rendu principal) selon le niveau de détail demandé, plutôt que de
/// dupliquer la logique de `render_node`/`render_children` — ces deux
/// représentations (texte indenté + liste plate `RenderedNode`) sont
/// produites en un seul parcours de l'arbre (voir `render_node`), donc
/// filtrer après coup ne recoûte aucun appel CDP supplémentaire.
fn render_at_detail_level(structured: &[RenderedNode], full_text: &str, detail: DetailLevel) -> String {
    match detail {
        DetailLevel::Full => full_text.to_string(),
        DetailLevel::Compact => structured
            .iter()
            .filter(|n| n.r#ref.is_some())
            .map(|n| format_rendered(n, None))
            .collect::<Vec<_>>()
            .join("\n"),
        DetailLevel::RefsOnly => structured
            .iter()
            .filter_map(|n| {
                let r = n.r#ref.as_ref()?;
                let flags = if n.state_flags.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", n.state_flags.join(", "))
                };
                if n.name.is_empty() {
                    Some(format!("[{r}] {}{flags}", n.role))
                } else {
                    Some(format!("[{r}] {} \"{}\"{flags}", n.role, n.name))
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Diff structurel par `backend_id` : compare deux captures nœud-par-nœud en
/// utilisant le `backendDOMNodeId` comme clé stable plutôt que le texte de
/// ligne. Élimine le défaut du diff textuel qu'il remplace (V2/V3) : les refs
/// `[eN]` étant un simple index de parcours recalculé à chaque capture, un
/// noeud ajouté/retiré n'importe où dans l'arbre décale toutes les refs
/// suivantes — une comparaison ligne-à-ligne peut alors soit bruiter sur des
/// lignes qui n'ont "que" changé de ref, soit, plus grave, masquer une vraie
/// suppression si le texte des lignes restantes se recombine par coïncidence
/// (observé en test isolé, voir commentaire sur `LAST_SNAPSHOT_STRUCT`).
///
/// Un `backend_id` présent avant et absent après = suppression. Absent avant
/// et présent après = ajout. Présent des deux côtés avec un `role`/`name`
/// différent = modification (ex: un compteur dont le texte change). Présent
/// des deux côtés à l'identique = inchangé, même si sa position dans l'ordre
/// de parcours (donc sa ref `[eN]`) a bougé entre les deux captures.
fn diff_structural(previous: &[RenderedNode], current: &[RenderedNode]) -> String {
    let prev_by_id: HashMap<i64, &RenderedNode> = previous.iter().map(|n| (n.backend_id, n)).collect();
    let curr_by_id: HashMap<i64, &RenderedNode> = current.iter().map(|n| (n.backend_id, n)).collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();
    let mut unchanged_count = 0usize;

    for node in current {
        match prev_by_id.get(&node.backend_id) {
            None => added.push(node),
            Some(prev_node) => {
                if prev_node.role != node.role
                    || prev_node.name != node.name
                    || prev_node.state_flags != node.state_flags
                {
                    modified.push((*prev_node, node));
                } else {
                    unchanged_count += 1;
                }
            }
        }
    }
    for node in previous {
        if !curr_by_id.contains_key(&node.backend_id) {
            removed.push(node);
        }
    }

    if added.is_empty() && removed.is_empty() && modified.is_empty() {
        return format!("(aucun changement détecté depuis le dernier snapshot — {unchanged_count} nœuds inchangés)");
    }

    let mut out = format!(
        "{unchanged_count} nœuds inchangés, {} ajoutés, {} retirés, {} modifiés depuis le dernier snapshot:\n",
        added.len(),
        removed.len(),
        modified.len()
    );
    for n in &modified {
        let (before, after) = n;
        out.push_str(&format!("~ {}\n", format_rendered(after, Some(before))));
    }
    for n in &added {
        out.push_str(&format!("+ {}\n", format_rendered(n, None)));
    }
    for n in &removed {
        out.push_str(&format!("- {}\n", format_rendered(n, None)));
    }
    out
}

fn format_rendered(node: &RenderedNode, before: Option<&RenderedNode>) -> String {
    let indent = "  ".repeat(node.depth as usize);
    let flags_suffix = if node.state_flags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", node.state_flags.join(", "))
    };
    let self_text = match &node.r#ref {
        Some(r) if node.name.is_empty() => format!("{indent}[{r}] {}{flags_suffix}", node.role),
        Some(r) => format!("{indent}[{r}] {} \"{}\"{flags_suffix}", node.role, node.name),
        None => format!("{indent}{}{flags_suffix}", node.role),
    };
    match before {
        Some(prev) if prev.name != node.name => format!("{self_text} (était \"{}\")", prev.name),
        Some(prev) if prev.state_flags != node.state_flags => {
            let prev_flags = if prev.state_flags.is_empty() {
                "aucun".to_string()
            } else {
                prev.state_flags.join(", ")
            };
            format!("{self_text} (était [{prev_flags}])")
        }
        _ => self_text,
    }
}

/// Seuil au-delà duquel des siblings répétitifs (lignes de tableau, items de
/// liste) sont compressés en motif + premier/dernier au lieu d'être tous
/// déroulés — un tableau ERP de 50 lignes produit ~300 noeuds quasi
/// identiques, dont l'essentiel du texte est redondant pour un agent qui a
/// juste besoin de savoir "il y a une table de N lignes avec ces colonnes,
/// voici les refs si besoin d'agir sur une ligne précise".
const REPEAT_COMPRESSION_THRESHOLD: usize = 8;

fn render_node(
    node: &RawAxNode,
    by_id: &HashMap<String, &RawAxNode>,
    depth: u32,
    out: &mut String,
    ref_entries: &mut Vec<(String, i64)>,
    index: &mut usize,
    structured: &mut Vec<RenderedNode>,
) {
    let is_noise = node.ignored || NOISE_ROLES.contains(&node.role.as_str());
    let is_interactive = is_interactive_role(&node.role);

    if !is_noise {
        let indent = "  ".repeat(depth as usize);
        let backend_id = node.backend_dom_node_id.unwrap_or(-1);
        let flags_suffix = if node.state_flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", node.state_flags.join(", "))
        };
        if is_interactive || !node.name.is_empty() {
            let r = make_ref(*index);
            *index += 1;
            ref_entries.push((r.clone(), backend_id));
            structured.push(RenderedNode {
                backend_id,
                role: node.role.clone(),
                name: node.name.clone(),
                depth,
                r#ref: Some(r.clone()),
                state_flags: node.state_flags.clone(),
            });
            if node.name.is_empty() {
                out.push_str(&format!("{indent}[{r}] {}{flags_suffix}\n", node.role));
            } else {
                out.push_str(&format!("{indent}[{r}] {} \"{}\"{flags_suffix}\n", node.role, node.name));
            }
        } else {
            structured.push(RenderedNode {
                backend_id,
                role: node.role.clone(),
                name: String::new(),
                depth,
                r#ref: None,
                state_flags: node.state_flags.clone(),
            });
            out.push_str(&format!("{indent}{}{flags_suffix}\n", node.role));
        }
    }

    let next_depth = if is_noise { depth } else { depth + 1 };
    render_children(&node.child_ids, by_id, next_depth, out, ref_entries, index, structured);
}

/// Rend les enfants d'un noeud, en détectant les runs de siblings consécutifs
/// de même rôle (au-delà de `REPEAT_COMPRESSION_THRESHOLD`) pour les
/// compresser en un résumé + premier/dernier élément détaillés, plutôt que de
/// tout dérouler à plat.
fn render_children(
    child_ids: &[String],
    by_id: &HashMap<String, &RawAxNode>,
    depth: u32,
    out: &mut String,
    ref_entries: &mut Vec<(String, i64)>,
    index: &mut usize,
    structured: &mut Vec<RenderedNode>,
) {
    let children: Vec<&RawAxNode> = child_ids.iter().filter_map(|cid| by_id.get(cid).copied()).collect();

    let mut i = 0;
    while i < children.len() {
        let role = &children[i].role;
        let mut run_end = i + 1;
        while run_end < children.len() && children[run_end].role == *role {
            run_end += 1;
        }
        let run_len = run_end - i;

        if run_len > REPEAT_COMPRESSION_THRESHOLD {
            let indent = "  ".repeat(depth as usize);
            out.push_str(&format!("{indent}[{run_len} × {role}, motif répété]\n"));

            // Premier élément détaillé (avec ses propres enfants, refs incluses).
            render_node(children[i], by_id, depth, out, ref_entries, index, structured);
            out.push_str(&format!("{indent}  ... {} éléments similaires omis ...\n", run_len - 2));
            // Dernier élément détaillé, pour donner une ref exploitable en fin de liste.
            render_node(children[run_end - 1], by_id, depth, out, ref_entries, index, structured);
        } else {
            for child in &children[i..run_end] {
                render_node(child, by_id, depth, out, ref_entries, index, structured);
            }
        }
        i = run_end;
    }
}

fn is_interactive_role(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "link"
            | "textbox"
            | "checkbox"
            | "radio"
            | "combobox"
            | "listbox"
            | "option"
            | "menuitem"
            | "tab"
            | "switch"
            | "slider"
            | "searchbox"
    )
}
