use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::sync::Mutex as AsyncMutex;

/// Registre global des tables de refs, indexé par target_id de page.
/// Une ref (`e12`) est scopée au dernier snapshot pris sur CETTE page: dès
/// qu'un nouveau snapshot est capturé, l'ancienne table est remplacée et
/// toute ref antérieure devient invalide (erreur stale_ref explicite plutôt
/// qu'un fallback silencieux sur le mauvais élément).
static REGISTRY: OnceLock<AsyncMutex<HashMap<String, HashMap<String, i64>>>> = OnceLock::new();

fn registry() -> &'static AsyncMutex<HashMap<String, HashMap<String, i64>>> {
    REGISTRY.get_or_init(|| AsyncMutex::new(HashMap::new()))
}

pub async fn store(page_target_id: &str, entries: Vec<(String, i64)>) {
    let mut reg = registry().lock().await;
    reg.insert(page_target_id.to_string(), entries.into_iter().collect());
}

pub async fn resolve(page_target_id: &str, r: &str) -> Option<i64> {
    let reg = registry().lock().await;
    reg.get(page_target_id).and_then(|m| m.get(r).copied())
}

/// Génère un identifiant court et stable pour un noeud dans un snapshot donné.
/// Format: e<index> — court pour économiser des tokens, unique dans le scope du snapshot.
pub fn make_ref(index: usize) -> String {
    format!("e{index}")
}
