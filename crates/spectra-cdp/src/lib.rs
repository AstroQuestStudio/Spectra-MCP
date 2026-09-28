pub mod actions;
pub mod console;
pub mod launcher;
pub mod ledger;
pub mod network;
pub mod observers;
pub mod params_types;
pub mod raw_ax;
pub mod refs;
pub mod session;
pub mod snapshot;

use anyhow::{bail, Result};
use chromiumoxide::page::Page;

pub use launcher::{launch_or_attach, LaunchMode, LaunchOutcome};
pub use session::BrowserSession;
pub use snapshot::{Snapshot, SnapshotNode};

/// Résout une ref de snapshot (`"e12"`) en backend_node_id CDP pour la page
/// donnée. Erreur explicite `stale_ref` si la ref n'existe pas dans le
/// dernier snapshot pris sur cette page (fail-fast plutôt qu'un fallback
/// silencieux sur un mauvais élément — voir refs.rs).
pub async fn resolve_ref(page: &Page, r: &str) -> Result<i64> {
    let target = page.target_id().inner().to_string();
    match refs::resolve(&target, r).await {
        Some(id) => Ok(id),
        None => bail!("stale_ref: la référence '{r}' n'existe pas dans le dernier snapshot de cette page — reprends un browser_snapshot"),
    }
}
