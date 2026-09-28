use crate::launcher::{launch_or_attach, LaunchMode, LaunchOutcome};
use anyhow::{Context, Result};
use chromiumoxide::browser::Browser;
use chromiumoxide::page::Page;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Une session Chrome pour un projet donné : possède (ou est attachée à) un
/// Browser et garde le registre des pages ouvertes. Les refs de snapshot et
/// les buffers console/réseau vivent dans des registres globaux indexés par
/// target_id de page (voir refs.rs/console.rs/network.rs) plutôt qu'ici, pour
/// rester accessibles depuis snapshot.rs/actions.rs sans avoir à faire
/// transiter la session à travers chaque fonction.
/// Le Drop garantit qu'un Chrome *spawn* par nous est bien tué à la fin —
/// jamais un Chrome auquel on s'est juste attaché (celui-là appartient à
/// l'utilisateur ou à une session précédente encore active).
pub struct BrowserSession {
    pub project_key: String,
    pub port: u16,
    pub profile_dir: PathBuf,
    /// true si ce process a spawn Chrome lui-même (donc responsable de le tuer).
    /// false si on s'est seulement attaché à une instance existante — dans ce
    /// cas `driver` ne doit JAMAIS être tué à la fin de la session.
    owns_process: bool,
    ws_url: String,
    driver: Mutex<Browser>,
    pages: Mutex<HashMap<String, Page>>,
    active_page_id: Mutex<Option<String>>,
}

impl BrowserSession {
    pub async fn launch(
        project_key: impl Into<String>,
        mode: LaunchMode,
        headless: bool,
    ) -> Result<Arc<Self>> {
        let project_key = project_key.into();
        let outcome: LaunchOutcome =
            launch_or_attach(&project_key, mode, headless, None).await?;

        let owns_process = outcome.spawned_child.is_some();
        let driver = match outcome.spawned_child {
            // Browser::launch nous a déjà rendu un Browser connecté et gérant
            // le process enfant — on le réutilise tel quel, pas de reconnexion.
            Some(owned) => owned,
            None => {
                // Le Handler retourné par connect() DOIT tourner en tâche de
                // fond: c'est lui qui pompe les réponses/évènements CDP sur la
                // websocket. Sans ça, tout appel CDP se bloque puis échoue en
                // "send failed because receiver is gone" dès que le canal
                // interne se ferme faute de lecteur.
                let (browser, handler) = Browser::connect(outcome.ws_url.clone())
                    .await
                    .context("attach sur l'instance Chrome existante")?;
                tokio::spawn(async move {
                    use futures::StreamExt;
                    let mut handler = handler;
                    while handler.next().await.is_some() {}
                });
                browser
            }
        };

        Ok(Arc::new(Self {
            project_key,
            port: outcome.port,
            profile_dir: outcome.profile_dir,
            owns_process,
            ws_url: outcome.ws_url,
            driver: Mutex::new(driver),
            pages: Mutex::new(HashMap::new()),
            active_page_id: Mutex::new(None),
        }))
    }

    pub fn was_attached_not_spawned(&self) -> bool {
        !self.owns_process
    }

    /// URL websocket CDP de cette session, utilisée par recipe-runner.mjs
    /// pour ouvrir sa propre connexion CDP côté Node (voir spectra-tools::recipe).
    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    /// Retourne l'id de la page active, en créant/adoptant le premier onglet si besoin.
    ///
    /// Quand ce process vient de spawn Chrome (`owns_process`), l'onglet natif
    /// "New Tab" que Chrome ouvre toujours lui-même au démarrage n'est pas
    /// forcément déjà visible via `driver.pages()` au moment du tout premier
    /// appel : son évènement CDP `Target.targetCreated` est traité de façon
    /// asynchrone par le handler, et peut ne pas encore être passé juste après
    /// `Browser::launch()`. Sans ce retry, on concluait à tort qu'aucune page
    /// n'existait et on en spawnait une seconde (`new_page`) — d'où les deux
    /// onglets observés en usage réel (un "New Tab" vide + un onglet Spectra),
    /// alors qu'un seul onglet par navigateur suffit et coûte moins cher.
    pub async fn ensure_active_page(&self) -> Result<String> {
        {
            let active = self.active_page_id.lock().await;
            if let Some(id) = active.as_ref() {
                return Ok(id.clone());
            }
        }

        let driver = self.driver.lock().await;
        let mut existing_pages = driver.pages().await?;
        if existing_pages.is_empty() && self.owns_process {
            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                existing_pages = driver.pages().await?;
                if !existing_pages.is_empty() {
                    break;
                }
            }
        }
        let page = if let Some(p) = existing_pages.into_iter().next() {
            p
        } else {
            driver.new_page("about:blank").await?
        };
        drop(driver);

        crate::observers::ensure_observing(&page).await.ok();

        let id = format!("p{}", page.target_id().inner());
        let mut pages = self.pages.lock().await;
        pages.insert(id.clone(), page);
        drop(pages);

        *self.active_page_id.lock().await = Some(id.clone());
        Ok(id)
    }

    pub async fn page_ids(&self) -> Vec<String> {
        self.pages.lock().await.keys().cloned().collect()
    }

    /// Ouvre un nouvel onglet et le rend actif — jusqu'ici jamais réellement
    /// implémenté malgré `browser_tabs(action="new")` documenté comme
    /// fonctionnel: le tool ignorait silencieusement `action` et retournait
    /// toujours juste la liste des pages existantes.
    pub async fn new_page(&self, url: &str) -> Result<String> {
        let driver = self.driver.lock().await;
        let page = driver.new_page(url).await?;
        drop(driver);

        crate::observers::ensure_observing(&page).await.ok();

        let id = format!("p{}", page.target_id().inner());
        let mut pages = self.pages.lock().await;
        pages.insert(id.clone(), page);
        drop(pages);

        *self.active_page_id.lock().await = Some(id.clone());
        Ok(id)
    }

    /// Ferme un onglet précis. Si c'était l'onglet actif, le prochain appel à
    /// `ensure_active_page` en adoptera un autre parmi ceux restants (ou en
    /// créera un si plus aucun n'existe).
    pub async fn close_page(&self, page_id: &str) -> Result<()> {
        let page = {
            let mut pages = self.pages.lock().await;
            pages.remove(page_id).with_context(|| format!("page inconnue: {page_id}"))?
        };
        page.close().await.ok();

        let mut active = self.active_page_id.lock().await;
        if active.as_deref() == Some(page_id) {
            *active = None;
        }
        Ok(())
    }

    /// Sélectionne un onglet existant comme onglet actif (celui utilisé par
    /// défaut quand un tool omet `page_id`).
    pub async fn select_page(&self, page_id: &str) -> Result<()> {
        let pages = self.pages.lock().await;
        if !pages.contains_key(page_id) {
            anyhow::bail!("page inconnue: {page_id}");
        }
        drop(pages);
        *self.active_page_id.lock().await = Some(page_id.to_string());
        Ok(())
    }

    pub async fn with_page<F, Fut, T>(&self, page_id: Option<&str>, f: F) -> Result<T>
    where
        F: FnOnce(Page) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let id = match page_id {
            Some(id) => id.to_string(),
            None => self.ensure_active_page().await?,
        };
        let page = {
            let pages = self.pages.lock().await;
            pages.get(&id).cloned().with_context(|| format!("page inconnue: {id}"))?
        };
        f(page).await
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        if !self.owns_process {
            // Attaché seulement: ce Chrome ne nous appartient pas, on ne le touche pas.
            return;
        }
        // Best-effort: Drop n'est pas async, on ne peut pas garantir le lock.
        // chromiumoxide tue déjà le process enfant dans le Drop de son propre
        // Child; on renforce avec taskkill /T pour emporter l'arbre de
        // sous-process (GPU/renderer/utility) qu'un simple kill laisserait
        // orphelin (c'est précisément la classe de bug documentée sur
        // playwright-mcp — 213 process orphelins recensés en issue upstream).
        if let Ok(mut driver) = self.driver.try_lock() {
            if let Some(child) = driver.get_mut_child() {
                if let Some(pid) = child_pid(child) {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/T", "/F", "/PID", &pid.to_string()])
                        .output();
                }
            }
        }
    }
}

fn child_pid(child: &chromiumoxide::async_process::Child) -> Option<u32> {
    Some(child.inner.id())
}
