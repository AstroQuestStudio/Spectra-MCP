use chromiumoxide_types::{Command, Method, MethodId};
use serde::Serialize;
use std::borrow::Cow;

/// Contournement d'un bug de couverture de `chromiumoxide_cdp 0.7.0` :
/// `AxNode.ignored_reasons` désérialise chaque raison via l'enum `AxPropertyName`,
/// qui ne couvre pas la catégorie CDP "raisons de masquage de nœud"
/// (`notRendered`, `notVisible`, `uninteresting`, etc). Résultat : n'importe
/// quelle page où Chrome ignore un nœud pour une de ces raisons fait échouer
/// TOUT `Accessibility.getFullAXTree` avec `serde_json::Error("uninteresting")`
/// -- même si la commande a réellement réussi côté navigateur.
///
/// On rejoue la même commande CDP mais en désérialisant la réponse en
/// `serde_json::Value` brut, pour lire nous-mêmes les seuls champs qui nous
/// intéressent (role, name, ignored, backendDOMNodeId, childIds, nodeId,
/// parentId) sans jamais tenter de parser `ignoredReasons`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct RawGetFullAxTree {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<i64>,
}

impl Method for RawGetFullAxTree {
    fn identifier(&self) -> MethodId {
        Cow::Borrowed("Accessibility.getFullAXTree")
    }
}

impl Command for RawGetFullAxTree {
    type Response = serde_json::Value;
}

/// Vue minimale et tolérante d'un noeud AX, lue directement depuis le JSON
/// brut renvoyé par Chrome — n'échoue jamais sur un champ qu'on ne connaît pas.
pub struct RawAxNode {
    pub node_id: String,
    pub parent_id: Option<String>,
    pub child_ids: Vec<String>,
    pub ignored: bool,
    pub role: String,
    pub name: String,
    pub backend_dom_node_id: Option<i64>,
    /// États d'interaction extraits de `AXNode.properties`, whitelist
    /// volontairement stricte (plutôt qu'une blacklist de bruit) — voir
    /// `extract_state_flags`. Rendus sous forme `[disabled, required, ...]`
    /// juste après le rôle/nom dans le snapshot, uniquement quand présents.
    pub state_flags: Vec<&'static str>,
}

pub fn parse_nodes(value: &serde_json::Value) -> Vec<RawAxNode> {
    let Some(nodes) = value.get("nodes").and_then(|n| n.as_array()) else {
        return Vec::new();
    };
    nodes.iter().map(parse_node).collect()
}

fn parse_node(n: &serde_json::Value) -> RawAxNode {
    let node_id = n.get("nodeId").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let parent_id = n.get("parentId").and_then(|v| v.as_str()).map(|s| s.to_string());
    let child_ids = n
        .get("childIds")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|c| c.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let ignored = n.get("ignored").and_then(|v| v.as_bool()).unwrap_or(false);
    let role = n
        .get("role")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = n
        .get("name")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let backend_dom_node_id = n.get("backendDOMNodeId").and_then(|v| v.as_i64());
    let state_flags = extract_state_flags(n);

    RawAxNode { node_id, parent_id, child_ids, ignored, role, name, backend_dom_node_id, state_flags }
}

/// Lit `AXNode.properties` (tableau de `{name, value: {type, value}}`) et n'en
/// retient que les propriétés d'état à fort signal pour un agent qui doit
/// décider s'il peut/doit interagir avec un élément — whitelist volontaire
/// plutôt que blacklist : `focusable` et `invalid=false` sont présents sur
/// quasi tous les nœuds (vérifié en spike contre AstroQuest et une page de
/// test dédiée) sans jamais être informatifs, les inclure gonflerait chaque
/// ligne du snapshot sans bénéfice. Chaque propriété n'apparaît que si sa
/// valeur diffère du silence implicite ("non coché", "activé", "replié" ne
/// sont jamais affichés — seul l'état qui mérite l'attention l'est).
///
/// Types CDP observés en pratique (spike isolé) : `boolean` (`disabled`,
/// `required`, `readonly`), `tristate` (`checked`, `pressed` — une *string*
/// `"true"/"false"/"mixed"`, pas un booléen JSON), `booleanOrUndefined`
/// (`expanded`, `selected` — absent du tableau si l'état ne s'applique pas au
/// rôle du nœud, ce que CDP encode en omettant la propriété plutôt qu'en la
/// mettant à `false`).
fn extract_state_flags(n: &serde_json::Value) -> Vec<&'static str> {
    let Some(props) = n.get("properties").and_then(|p| p.as_array()) else {
        return Vec::new();
    };
    let mut flags = Vec::new();
    for p in props {
        let Some(name) = p.get("name").and_then(|v| v.as_str()) else { continue };
        let value = p.get("value").and_then(|v| v.get("value"));
        let is_true = matches!(value.and_then(|v| v.as_bool()), Some(true))
            || matches!(value.and_then(|v| v.as_str()), Some("true"));
        let is_mixed = matches!(value.and_then(|v| v.as_str()), Some("mixed"));
        match name {
            "disabled" if is_true => flags.push("disabled"),
            "required" if is_true => flags.push("required"),
            "readonly" if is_true => flags.push("readonly"),
            "checked" if is_true => flags.push("checked"),
            "checked" if is_mixed => flags.push("checked:mixed"),
            "pressed" if is_true => flags.push("pressed"),
            "expanded" if is_true => flags.push("expanded"),
            "selected" if is_true => flags.push("selected"),
            _ => {}
        }
    }
    flags
}
