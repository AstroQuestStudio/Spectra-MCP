use crate::params::*;
use crate::recipe;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use spectra_cdp::launcher::LaunchMode;
use spectra_cdp::BrowserSession;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Nombre maximum de sessions Chrome simultanées gérées par CE process serveur
/// (tous projets confondus). Protège la machine d'un usage runaway (un agent
/// qui spawn un Chrome par erreur à chaque appel). Volontairement > 1 : le but
/// est aussi de permettre plusieurs navigateurs en parallèle pour simuler du
/// multi-utilisateur (ex: tester une édition collaborative à plusieurs sur un
/// même document). Configurable via la variable d'environnement
/// SPECTRA_MAX_SESSIONS ; 6 est un compromis raisonnable par défaut (RAM d'un
/// Chrome headless ≈ 150-250 Mo, donc jusqu'à ~1.5 Go pour le pire cas).
const DEFAULT_MAX_SESSIONS: usize = 6;

fn max_sessions() -> usize {
    std::env::var("SPECTRA_MAX_SESSIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or(DEFAULT_MAX_SESSIONS)
}

/// Durée après laquelle un Chrome pré-chauffé jamais consommé par un vrai
/// browser_launch/browser_navigate est fermé automatiquement (voir
/// `warm_slot` et `spawn_warmup`). Volontairement courte : le pré-chauffage
/// ne vaut la peine que si le premier appel outil arrive dans les toutes
/// premières secondes du handshake MCP: passé ce délai, le gain de latence
/// n'a plus de sens à protéger contre le risque de fuite.
const WARMUP_TTL_SECS: u64 = 20;

/// Point d'entrée de tous les tools MCP Spectra. Garde une session Chrome par
/// clé de projet (un seul serveur peut piloter plusieurs projets en parallèle,
/// chacun avec son propre Chrome/port/profil — voir spectra-cdp::launcher).
/// Réutilise toujours la session existante pour une même clé de projet: un
/// second browser_launch sur le même projet ne spawn jamais de second Chrome.
///
/// Note V2→V3 : un pool de Chrome "pré-chauffé" au démarrage du serveur a
/// d'abord été tenté sans garde-fou (spawn immédiat dès `new()` pour le
/// projet par défaut) puis abandonné — dans les cas où le premier
/// `browser_launch` ciblait un projet différent du cwd, ce Chrome n'était
/// jamais consommé et fuyait indéfiniment tant que le serveur tournait.
/// Repris ici avec un TTL strict (`WARMUP_TTL_SECS`) : le warm-up est fermé
/// automatiquement s'il n'a pas été consommé dans ce délai, bornant le risque
/// de fuite à une fenêtre courte et mesurable plutôt que de le laisser
/// indéfini.
#[derive(Clone)]
pub struct SpectraTools {
    sessions: Arc<Mutex<HashMap<String, Arc<BrowserSession>>>>,
    default_project: Arc<Mutex<Option<String>>>,
    /// Session pré-chauffée pour le cwd du serveur, en attente d'être
    /// consommée par le premier `session_for()` qui matche sa clé de projet.
    /// `None` une fois consommée (déplacée dans `sessions`) ou après
    /// expiration du TTL (fermée par la tâche de fond de `spawn_warmup`).
    warm_slot: Arc<Mutex<Option<(String, Arc<BrowserSession>)>>>,
    tool_router: ToolRouter<Self>,
}

impl Default for SpectraTools {
    fn default() -> Self {
        Self::new()
    }
}

impl SpectraTools {
    pub fn new() -> Self {
        let tools = Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            default_project: Arc::new(Mutex::new(None)),
            warm_slot: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        };
        // Pré-chauffage désormais opt-in (SPECTRA_PREWARM=1) plutôt que
        // systématique : un serveur qui vient de démarrer (handshake MCP) ne
        // doit jamais spawn Chrome de son propre chef — seul un vrai
        // browser_launch/browser_navigate explicite doit ouvrir un
        // navigateur. Avant ce changement, chaque démarrage du serveur (donc
        // chaque redémarrage de Claude Code) ouvrait un Chrome silencieusement,
        // même si la session ne finissait jamais par piloter de navigateur.
        if std::env::var("SPECTRA_PREWARM").as_deref() == Ok("1") {
            tools.spawn_warmup();
        }
        tools
    }

    /// Lance en tâche de fond le pré-chauffage d'un Chrome pour le cwd du
    /// serveur, sans bloquer le handshake MCP (browser_launch reste
    /// disponible immédiatement même si ce spawn est encore en cours — il
    /// retombera juste sur le chemin normal, sans attendre le warm-up).
    /// Best-effort total : toute erreur ici (Chrome absent, port pris) est
    /// silencieuse, le chemin normal de `session_for` prend le relais.
    /// Actif uniquement si SPECTRA_PREWARM=1 (voir `new`).
    fn spawn_warmup(&self) {
        let warm_slot = self.warm_slot.clone();
        tokio::spawn(async move {
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "default".to_string());

            let Ok(session) = BrowserSession::launch(cwd.clone(), LaunchMode::Auto, false).await
            else {
                return;
            };

            {
                let mut slot = warm_slot.lock().await;
                *slot = Some((cwd, session));
            }

            tokio::time::sleep(std::time::Duration::from_secs(WARMUP_TTL_SECS)).await;

            // Si toujours présent après le TTL, personne ne l'a consommé —
            // on le retire du slot et on laisse le Drop fermer le Chrome
            // sous-jacent. Si un session_for() l'a déjà consommé entre
            // temps, le slot est à None et cette passe est un no-op: aucune
            // session active n'est jamais touchée par ce nettoyage.
            let mut slot = warm_slot.lock().await;
            if slot.is_some() {
                *slot = None;
            }
        });
    }

    async fn resolve_project_key(&self, explicit: Option<String>) -> String {
        if let Some(p) = explicit {
            return p;
        }
        let mut default = self.default_project.lock().await;
        if let Some(p) = default.as_ref() {
            return p.clone();
        }
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "default".to_string());
        *default = Some(cwd.clone());
        cwd
    }

    async fn session_for(&self, project: Option<String>) -> anyhow::Result<Arc<BrowserSession>> {
        self.session_for_with_options(project, LaunchMode::Auto, false).await
    }

    /// Comme `session_for`, mais permet de préciser `mode`/`headless` — pris
    /// en compte uniquement si une nouvelle session doit réellement être
    /// spawnée (une session déjà existante, ou le warm-up consommé, gardent
    /// leurs propres options d'origine, on ne relance jamais Chrome pour
    /// changer ces réglages en cours de route).
    ///
    /// Corrige un bug réel : `browser_launch` acceptait `mode`/`headless`
    /// dans son schéma MCP (documentés dans le README) mais ces deux champs
    /// n'étaient jamais lus — `session_for` appelait toujours
    /// `BrowserSession::launch(key, LaunchMode::Auto, false)` en dur, ce qui
    /// signifiait concrètement que `headless=true` n'avait jamais eu d'effet
    /// et que `mode="attach"` strict ne pouvait jamais échouer comme prévu
    /// (vérifié : un Chrome démarrait quand même). Découvert en testant
    /// explicitement ces deux paramètres, jamais vérifiés jusqu'ici.
    async fn session_for_with_options(
        &self,
        project: Option<String>,
        mode: LaunchMode,
        headless: bool,
    ) -> anyhow::Result<Arc<BrowserSession>> {
        let key = self.resolve_project_key(project).await;
        let mut sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get(&key) {
            return Ok(s.clone());
        }

        // Consomme le warm-up pré-chauffé s'il correspond à ce projet et n'a
        // pas encore expiré (voir spawn_warmup) — évite de spawn un second
        // Chrome pour la même clé alors qu'un premier tourne déjà en attente.
        {
            let mut warm = self.warm_slot.lock().await;
            if let Some((warm_key, warm_session)) = warm.take() {
                if warm_key == key {
                    sessions.insert(key, warm_session.clone());
                    return Ok(warm_session);
                }
                // Ne correspond pas à ce projet: on le remet en place, une
                // autre requête (ou le TTL) s'en chargera.
                *warm = Some((warm_key, warm_session));
            }
        }

        let limit = max_sessions();
        if sessions.len() >= limit {
            anyhow::bail!(
                "limite de {limit} sessions Chrome simultanées atteinte (projets actifs: {}). \
                 Ferme une session existante avec browser_sessions(action=\"close\", project=\"...\") \
                 avant d'en ouvrir une nouvelle, ou augmente SPECTRA_MAX_SESSIONS si tu as \
                 réellement besoin de plus de navigateurs en parallèle (ex: simuler du multi-utilisateur).",
                sessions.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        }
        let session = BrowserSession::launch(key.clone(), mode, headless).await?;
        sessions.insert(key, session.clone());
        Ok(session)
    }
}

#[tool_router]
impl SpectraTools {
    #[tool(description = "Démarre ou rattache Chrome pour le projet courant. À appeler une fois avant les autres tools (les autres tools l'appellent aussi implicitement avec les valeurs par défaut).")]
    async fn browser_launch(&self, Parameters(p): Parameters<LaunchParams>) -> String {
        // Un projet explicite devient le nouveau défaut pour les tools
        // suivants sans qu'il soit nécessaire de le repréciser à chaque
        // appel (browser_navigate, browser_click, ... appellent tous
        // session_for(None) en interne) — cohérent avec le fait que
        // l'utilisateur vient de désigner explicitement ce projet.
        if let Some(project) = p.project.clone() {
            *self.default_project.lock().await = Some(project);
        }
        let mode = match p.mode.as_deref() {
            Some("attach") => LaunchMode::Attach,
            Some("spawn") => LaunchMode::Spawn,
            _ => LaunchMode::Auto,
        };
        let headless = p.headless.unwrap_or(false);
        match self.session_for_with_options(p.project.clone(), mode, headless).await {
            Ok(session) => {
                let pages = session.page_ids().await;
                serde_json::json!({
                    "status": "ok",
                    "port": session.port,
                    "profile_dir": session.profile_dir.to_string_lossy(),
                    "attached_existing": session.was_attached_not_spawned(),
                    "pages": pages,
                })
                .to_string()
            }
            Err(e) => error_json(&e),
        }
    }

    #[tool(description = "Capture l'arbre d'accessibilité condensé de la page active (texte hiérarchique avec refs [eN] pour cibler les éléments dans les tools suivants). Avec diff_only=true, ne retourne qu'un résumé des nœuds changés depuis le snapshot précédent de cette page. Avec check_a11y=true, ajoute une liste a11y_issues de problèmes d'accessibilité basiques détectés. Avec detail=\"compact\", retire les nœuds purement structurels sans ref (gain de tokens sur les pages denses) ; avec detail=\"refs_only\", ne retourne qu'une ligne par ref sans hiérarchie (minimum vital pour cibler un clic). Avec root_ref=\"eN\" (ref d'un snapshot précédent), ne capture que le sous-arbre de cet élément (ex: une modale ouverte) au lieu de la page entière — gain de tokens supplémentaire sur les pages denses au-delà de detail seul.")]
    async fn browser_snapshot(&self, Parameters(p): Parameters<PageScopedParams>) -> String {
        let diff_only = p.diff_only.unwrap_or(false);
        let check_a11y = p.check_a11y.unwrap_or(false);
        let detail = spectra_cdp::snapshot::DetailLevel::parse(p.detail.as_deref());
        let root_ref = p.root_ref.clone();
        self.with_session_and_page(None, p.page_id, move |page| async move {
            match root_ref {
                Some(r) => {
                    let backend_id = spectra_cdp::resolve_ref(&page, &r).await?;
                    spectra_cdp::snapshot::capture_scoped(&page, diff_only, check_a11y, detail, backend_id).await
                }
                None => spectra_cdp::snapshot::capture_full(&page, diff_only, check_a11y, detail).await,
            }
        })
        .await
    }

    #[tool(description = "Navigue vers une URL et attend le chargement. Retourne le nouveau snapshot.")]
    async fn browser_navigate(&self, Parameters(p): Parameters<NavigateParams>) -> String {
        use anyhow::Context;
        let url = p.url.clone();
        let wait_networkidle = p.wait_until.as_deref() == Some("networkidle");

        let project_key = self.resolve_project_key(None).await;
        spectra_cdp::ledger::ledger_for(&project_key).await.lock().await.record_navigate(url.clone());

        self.with_session_and_page(None, p.page_id.clone(), move |page| async move {
            page.goto(&url).await.with_context(|| format!("page.goto({url}) a échoué"))?;
            page.wait_for_navigation().await.ok();
            if wait_networkidle {
                wait_network_idle(&page, 500, 4000).await;
            }
            // Attend que le DOM applicatif ait un contenu substantiel plutôt
            // qu'un sleep fixe: un load CDP ne garantit rien sur l'état d'une
            // SPA React/Vue qui hydrate juste après goto(). Mesuré sur une
            // vraie SPA (Vite+React, cold start): stabilisation vers ~2.6s
            // après goto() — le timeout doit couvrir ce cas sans pour autant
            // pénaliser les pages qui montent vite (elles sortent dès que
            // stable, bien avant le timeout).
            wait_dom_settled(&page, 4000).await;
            spectra_cdp::snapshot::capture(&page).await.context("capture du snapshot post-navigation a échoué")
        })
        .await
    }

    #[tool(description = "Clique sur un élément référencé par une ref de browser_snapshot. Avec diff_only=true, retourne un résumé compact du changement plutôt que l'arbre complet.")]
    async fn browser_click(&self, Parameters(p): Parameters<RefParams>) -> String {
        let diff_only = p.diff_only.unwrap_or(false);
        self.act_on_ref(p.page_id, &p.r#ref, diff_only, |page, backend_id| async move {
            spectra_cdp::actions::click_backend_node(&page, backend_id).await
        })
        .await
    }

    #[tool(description = "Saisit du texte dans un champ référencé, avec option de soumission (Entrée). Avec diff_only=true, retourne un résumé compact du changement plutôt que l'arbre complet.")]
    async fn browser_type(&self, Parameters(p): Parameters<TypeParams>) -> String {
        let text = p.text.clone();
        let submit = p.submit.unwrap_or(false);
        let diff_only = p.diff_only.unwrap_or(false);
        self.act_on_ref(p.page_id, &p.r#ref, diff_only, move |page, backend_id| async move {
            spectra_cdp::actions::type_into_backend_node(&page, backend_id, &text, submit).await
        })
        .await
    }

    #[tool(description = "Remplit plusieurs champs en un seul appel (réduit les allers-retours par rapport à browser_type répété). Avec diff_only=true, le snapshot retourné est un résumé compact du changement plutôt que l'arbre complet.")]
    async fn browser_fill_form(&self, Parameters(p): Parameters<FillFormParams>) -> String {
        let page_id = p.page_id.clone();
        let diff_only = p.diff_only.unwrap_or(false);
        let fields: Vec<spectra_cdp::params_types::FieldValue> = p
            .fields
            .into_iter()
            .map(|f| spectra_cdp::params_types::FieldValue { r#ref: f.r#ref, value: f.value })
            .collect();
        match self.session_for(None).await {
            Ok(session) => {
                let result = session
                    .with_page(page_id.as_deref(), move |page| async move {
                        spectra_cdp::actions::fill_form(&page, &fields, diff_only).await
                    })
                    .await;
                match result {
                    Ok(v) => v.to_string(),
                    Err(e) => error_json(&e),
                }
            }
            Err(e) => error_json(&e),
        }
    }

    #[tool(description = "Sélectionne une option dans un élément <select>. Avec diff_only=true, retourne un résumé compact du changement plutôt que l'arbre complet.")]
    async fn browser_select_option(&self, Parameters(p): Parameters<SelectOptionParams>) -> String {
        let value = p.value.clone();
        let diff_only = p.diff_only.unwrap_or(false);
        self.act_on_ref(p.page_id, &p.r#ref, diff_only, move |page, backend_id| async move {
            spectra_cdp::actions::select_option_backend_node(&page, backend_id, &value).await
        })
        .await
    }

    #[tool(description = "Survole un élément (utile pour révéler menus/tooltips). Avec diff_only=true, retourne un résumé compact du changement plutôt que l'arbre complet.")]
    async fn browser_hover(&self, Parameters(p): Parameters<RefParams>) -> String {
        let diff_only = p.diff_only.unwrap_or(false);
        self.act_on_ref(p.page_id, &p.r#ref, diff_only, |page, backend_id| async move {
            spectra_cdp::actions::hover_backend_node(&page, backend_id).await
        })
        .await
    }

    #[tool(description = "Capture un screenshot (page entière, viewport, ou élément précis). Réservé aux vérifications visuelles — préférer browser_snapshot pour les actions.")]
    async fn browser_screenshot(&self, Parameters(p): Parameters<ScreenshotParams>) -> String {
        let full_page = p.full_page.unwrap_or(false);
        let format = p.format.unwrap_or_else(|| "png".to_string());
        let r = p.r#ref.clone();
        match self.session_for(None).await {
            Ok(session) => {
                let result = session
                    .with_page(p.page_id.as_deref(), |page| async move {
                        spectra_cdp::actions::screenshot(&page, r.as_deref(), full_page, &format).await
                    })
                    .await;
                match result {
                    Ok(v) => v,
                    Err(e) => error_json(&e),
                }
            }
            Err(e) => error_json(&e),
        }
    }

    #[tool(description = "Attend qu'un texte apparaisse, qu'une ref existe, ou un délai. Retourne le snapshot une fois la condition remplie ou le timeout atteint.")]
    async fn browser_wait_for(&self, Parameters(p): Parameters<WaitForParams>) -> String {
        let timeout_ms = p.timeout_ms.unwrap_or(5000);
        let text = p.text.clone();
        let r#ref = p.r#ref.clone();
        match self.session_for(None).await {
            Ok(session) => {
                let result = session
                    .with_page(p.page_id.as_deref(), |page| async move {
                        spectra_cdp::actions::wait_for(&page, text.as_deref(), r#ref.as_deref(), timeout_ms).await
                    })
                    .await;
                match result {
                    Ok(v) => v,
                    Err(e) => error_json(&e),
                }
            }
            Err(e) => error_json(&e),
        }
    }

    #[tool(description = "Récupère les messages console (dédupliqués avec compteur d'occurrences) depuis le dernier appel ou un timestamp donné.")]
    async fn browser_console_messages(&self, Parameters(p): Parameters<ConsoleMessagesParams>) -> String {
        let level = match p.level.as_deref() {
            Some("error") => Some(spectra_cdp::console::ConsoleLevel::Error),
            Some("warning") => Some(spectra_cdp::console::ConsoleLevel::Warning),
            _ => None,
        };
        match self.session_for(None).await {
            Ok(session) => {
                let result = session
                    .with_page(p.page_id.as_deref(), move |page| async move {
                        let messages = match spectra_cdp::observers::console_buffer_for(&page).await {
                            Some(buf) => buf.lock().await.messages(level, p.since_ts),
                            None => Vec::new(),
                        };
                        Ok(serde_json::json!({ "messages": messages }))
                    })
                    .await;
                match result {
                    Ok(v) => v.to_string(),
                    Err(e) => error_json(&e),
                }
            }
            Err(e) => error_json(&e),
        }
    }

    #[tool(description = "Liste les requêtes réseau groupées par domaine et statut (pas de détail par défaut — voir browser_network_request_detail).")]
    async fn browser_network_requests(&self, Parameters(p): Parameters<NetworkRequestsParams>) -> String {
        let filter = spectra_cdp::network::StatusFilter::parse(p.status_filter.as_deref());
        match self.session_for(None).await {
            Ok(session) => {
                let result = session
                    .with_page(p.page_id.as_deref(), move |page| async move {
                        let groups = match spectra_cdp::observers::network_buffer_for(&page).await {
                            Some(buf) => buf.lock().await.grouped(filter),
                            None => Vec::new(),
                        };
                        Ok(serde_json::json!({ "groups": groups }))
                    })
                    .await;
                match result {
                    Ok(v) => v.to_string(),
                    Err(e) => error_json(&e),
                }
            }
            Err(e) => error_json(&e),
        }
    }

    #[tool(description = "Détail complet d'une requête réseau précise (headers, corps) par son request_id.")]
    async fn browser_network_request_detail(&self, Parameters(p): Parameters<NetworkRequestDetailParams>) -> String {
        match self.session_for(None).await {
            Ok(session) => {
                let result = session
                    .with_page(p.page_id.as_deref(), move |page| async move {
                        let record = match spectra_cdp::observers::network_buffer_for(&page).await {
                            Some(buf) => buf.lock().await.get(&p.request_id).cloned(),
                            None => None,
                        };
                        match record {
                            Some(r) => Ok(serde_json::to_value(r)?),
                            None => Ok(serde_json::json!({ "error": "request_id inconnu ou expiré" })),
                        }
                    })
                    .await;
                match result {
                    Ok(v) => v.to_string(),
                    Err(e) => error_json(&e),
                }
            }
            Err(e) => error_json(&e),
        }
    }

    #[tool(description = "Exécute une expression JavaScript dans le contexte de la page et retourne sa valeur sérialisée.")]
    async fn browser_evaluate(&self, Parameters(p): Parameters<EvaluateParams>) -> String {
        let expr = p.expression.clone();
        match self.session_for(None).await {
            Ok(session) => {
                let result = session
                    .with_page(p.page_id.as_deref(), |page| async move {
                        spectra_cdp::actions::evaluate(&page, &expr).await
                    })
                    .await;
                match result {
                    Ok(v) => v,
                    Err(e) => error_json(&e),
                }
            }
            Err(e) => error_json(&e),
        }
    }

    #[tool(description = "Mesure les Core Web Vitals (LCP, CLS, FCP, TTFB) de la page active. LCP/CLS proviennent d'un PerformanceObserver posé automatiquement dès le premier chargement de la page ; si la page a été ouverte avant l'activation de Spectra ou vient d'être rechargée à l'instant, laisse-lui quelques secondes puis rappelle ce tool.")]
    async fn browser_performance_metrics(&self, Parameters(p): Parameters<PerformanceMetricsParams>) -> String {
        match self.session_for(None).await {
            Ok(session) => {
                let result = session
                    .with_page(p.page_id.as_deref(), |page| async move {
                        spectra_cdp::actions::performance_metrics(&page).await
                    })
                    .await;
                match result {
                    Ok(v) => v.to_string(),
                    Err(e) => error_json(&e),
                }
            }
            Err(e) => error_json(&e),
        }
    }

    #[tool(description = "Liste, ouvre, ferme ou sélectionne un onglet.")]
    async fn browser_tabs(&self, Parameters(p): Parameters<TabsParams>) -> String {
        let session = match self.session_for(None).await {
            Ok(s) => s,
            Err(e) => return error_json(&e),
        };

        let result = match p.action.as_str() {
            "new" => {
                let url = p.url.as_deref().unwrap_or("about:blank");
                session.new_page(url).await.map(|id| serde_json::json!({ "opened": id }))
            }
            "close" => match &p.page_id {
                Some(id) => session.close_page(id).await.map(|_| serde_json::json!({ "closed": id })),
                None => Err(anyhow::anyhow!("action=\"close\" nécessite page_id")),
            },
            "select" => match &p.page_id {
                Some(id) => session.select_page(id).await.map(|_| serde_json::json!({ "selected": id })),
                None => Err(anyhow::anyhow!("action=\"select\" nécessite page_id")),
            },
            _ => Ok(serde_json::json!({})),
        };

        match result {
            Ok(extra) => {
                let ids = session.page_ids().await;
                let mut out = serde_json::json!({ "action": p.action, "pages": ids });
                if let (Some(out_map), Some(extra_map)) = (out.as_object_mut(), extra.as_object()) {
                    for (k, v) in extra_map {
                        out_map.insert(k.clone(), v.clone());
                    }
                }
                out.to_string()
            }
            Err(e) => error_json(&e),
        }
    }

    #[tool(description = "Upload un ou plusieurs fichiers vers un <input type=file> référencé.")]
    async fn browser_file_upload(&self, Parameters(p): Parameters<FileUploadParams>) -> String {
        let paths = p.paths.clone();
        self.act_on_ref(p.page_id, &p.r#ref, false, move |page, backend_id| async move {
            spectra_cdp::actions::upload_files(&page, backend_id, &paths).await
        })
        .await
    }

    #[tool(description = "Exécute une recipe .mjs du projet (<projet>/.spectra/recipes/<nom>.mjs) — mécanisme d'extensibilité par projet sans recompilation du serveur.")]
    async fn browser_run_recipe(&self, Parameters(p): Parameters<RunRecipeParams>) -> String {
        match self.session_for(Some(p.project.clone())).await {
            Ok(session) => match recipe::run(&session, &p.project, &p.recipe, p.args.clone()).await {
                Ok(v) => v.to_string(),
                Err(e) => error_json(&e),
            },
            Err(e) => error_json(&e),
        }
    }

    #[tool(description = "Liste les sessions Chrome actives sur ce serveur (une par projet), ferme une session précise, ou ferme TOUTES les sessions d'un coup (action=\"close_all\") — utile pour un nettoyage rapide après des tests. Permet aussi de gérer plusieurs navigateurs en parallèle (ex: simuler plusieurs utilisateurs) sans dépasser la limite SPECTRA_MAX_SESSIONS.")]
    async fn browser_sessions(&self, Parameters(p): Parameters<SessionsParams>) -> String {
        match p.action.as_str() {
            "close" => {
                let Some(project) = p.project.clone() else {
                    return error_json(&anyhow::anyhow!("action=\"close\" nécessite project"));
                };
                let mut sessions = self.sessions.lock().await;
                match sessions.remove(&project) {
                    // Le Drop de BrowserSession (déclenché ici, dernière
                    // référence lâchée) ferme le process Chrome spawn.
                    Some(_) => serde_json::json!({ "closed": project }).to_string(),
                    None => error_json(&anyhow::anyhow!("aucune session active pour le projet '{project}'")),
                }
            }
            "close_all" => {
                let mut sessions = self.sessions.lock().await;
                let mut closed: Vec<String> = sessions.keys().cloned().collect();
                sessions.clear();
                // Le warm-up pré-chauffé (s'il n'a pas encore été consommé)
                // est un Chrome vivant comme les autres — close_all doit
                // aussi le fermer, sinon il pourrait survivre à une demande
                // explicite de tout arrêter.
                let mut warm = self.warm_slot.lock().await;
                if let Some((warm_key, _)) = warm.take() {
                    closed.push(format!("{warm_key} (warm-up)"));
                }
                serde_json::json!({ "closed": closed, "count": closed.len() }).to_string()
            }
            _ => {
                let sessions = self.sessions.lock().await;
                let mut list: Vec<_> = sessions
                    .iter()
                    .map(|(k, s)| serde_json::json!({ "project": k, "port": s.port }))
                    .collect();
                if let Some((warm_key, warm_session)) = self.warm_slot.lock().await.as_ref() {
                    list.push(serde_json::json!({
                        "project": warm_key,
                        "port": warm_session.port,
                        "warm_up_pending": true,
                    }));
                }
                serde_json::json!({ "active_sessions": list, "max_sessions": max_sessions() }).to_string()
            }
        }
    }

    #[tool(description = "Compile le journal de la session (navigations, actions, erreurs) en un rapport structuré — pour avoir une vue d'ensemble sans devoir se souvenir de tout l'historique de la conversation.")]
    async fn browser_report(&self, Parameters(p): Parameters<ReportParams>) -> String {
        let project_key = self.resolve_project_key(p.project.clone()).await;
        let ledger = spectra_cdp::ledger::ledger_for(&project_key).await;
        let ledger = ledger.lock().await;
        let events = ledger.events_since(p.since_ts);

        let mut navigations = 0u32;
        let mut actions_ok = 0u32;
        let mut actions_failed = 0u32;
        let mut errors: Vec<&spectra_cdp::ledger::LedgerEvent> = Vec::new();

        for e in &events {
            match e {
                spectra_cdp::ledger::LedgerEvent::Navigate { .. } => navigations += 1,
                spectra_cdp::ledger::LedgerEvent::Action { ok, .. } => {
                    if *ok {
                        actions_ok += 1;
                    } else {
                        actions_failed += 1;
                    }
                }
                spectra_cdp::ledger::LedgerEvent::Error { .. } => errors.push(e),
            }
        }

        serde_json::json!({
            "project": project_key,
            "summary": {
                "navigations": navigations,
                "actions_ok": actions_ok,
                "actions_failed": actions_failed,
                "errors": errors.len(),
            },
            "events": events,
        })
        .to_string()
    }

    #[tool(description = "Exécute une séquence d'actions (click/type/select_option) sans repasser par un tour de raisonnement entre chaque étape. S'arrête au premier échec sauf si on_error=\"skip_and_continue\" pour cette étape. Réduit un formulaire de N champs à un seul appel MCP dans le cas nominal. Avec diff_only=true, ajoute un résumé compact du changement (capturé une seule fois après la dernière étape) au résultat.")]
    async fn browser_act_sequence(&self, Parameters(p): Parameters<ActSequenceParams>) -> String {
        let project_key = self.resolve_project_key(None).await;
        let session = match self.session_for(None).await {
            Ok(s) => s,
            Err(e) => return error_json(&e),
        };

        let mut completed = Vec::new();
        let mut blocked_at: Option<serde_json::Value> = None;
        let page_id = p.page_id.clone();

        for (idx, action) in p.actions.iter().enumerate() {
            let label = action.label.clone().unwrap_or_else(|| format!("step_{idx}"));
            let skip_on_error = action.on_error.as_deref() == Some("skip_and_continue");

            let result = session
                .with_page(page_id.as_deref(), {
                    let action = action.clone_for_exec();
                    move |page| async move { exec_sequence_action(&page, &action).await }
                })
                .await;

            match result {
                Ok(()) => {
                    completed.push(serde_json::json!({ "index": idx, "label": label, "result": "ok" }));
                }
                Err(e) => {
                    let ledger = spectra_cdp::ledger::ledger_for(&project_key).await;
                    ledger.lock().await.record_error(format!("act_sequence[{idx}] {label}: {e}"));
                    if skip_on_error {
                        completed.push(
                            serde_json::json!({ "index": idx, "label": label, "result": "skipped", "error": e.to_string() }),
                        );
                        continue;
                    }
                    blocked_at = Some(
                        serde_json::json!({ "index": idx, "label": label, "reason": e.to_string() }),
                    );
                    break;
                }
            }
        }

        let status = if blocked_at.is_some() { "partial_failure" } else { "ok" };

        let snapshot = if p.diff_only.unwrap_or(false) {
            let result = session.with_page(page_id.as_deref(), |page| async move {
                spectra_cdp::snapshot::capture_with_diff(&page, true).await
            }).await;
            result.ok()
        } else {
            None
        };

        serde_json::json!({
            "status": status,
            "completed": completed,
            "blocked_at": blocked_at,
            "snapshot": snapshot,
        })
        .to_string()
    }
}

impl SequenceAction {
    fn clone_for_exec(&self) -> SequenceActionExec {
        SequenceActionExec {
            op: self.op.clone(),
            r#ref: self.r#ref.clone(),
            value: self.value.clone(),
        }
    }
}

struct SequenceActionExec {
    op: String,
    r#ref: String,
    value: Option<String>,
}

async fn exec_sequence_action(page: &chromiumoxide::page::Page, action: &SequenceActionExec) -> anyhow::Result<()> {
    let backend_id = spectra_cdp::resolve_ref(page, &action.r#ref).await?;
    match action.op.as_str() {
        "click" => spectra_cdp::actions::click_backend_node(page, backend_id).await,
        "type" => {
            let value = action.value.clone().unwrap_or_default();
            spectra_cdp::actions::type_into_backend_node(page, backend_id, &value, false).await
        }
        "select_option" => {
            let value = action.value.clone().unwrap_or_default();
            spectra_cdp::actions::select_option_backend_node(page, backend_id, &value).await
        }
        other => anyhow::bail!("opération de séquence inconnue: '{other}' (attendu: click, type, select_option)"),
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SpectraTools {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Spectra: pilotage Chrome direct via CDP pour tester/inspecter des sites web. \
                 Commence par browser_snapshot pour voir l'état de la page (refs [eN]), puis \
                 utilise ces refs avec browser_click/browser_type/etc. browser_screenshot est \
                 réservé aux vérifications visuelles."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

impl SpectraTools {
    async fn with_session_and_page<F, Fut>(&self, project: Option<String>, page_id: Option<String>, f: F) -> String
    where
        F: FnOnce(chromiumoxide::page::Page) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<spectra_cdp::Snapshot>>,
    {
        match self.session_for(project).await {
            Ok(session) => {
                let result = session.with_page(page_id.as_deref(), f).await;
                match result {
                    Ok(snapshot) => serde_json::to_string(&snapshot).unwrap_or_default(),
                    Err(e) => error_json(&e),
                }
            }
            Err(e) => error_json(&e),
        }
    }

    async fn act_on_ref<F, Fut>(&self, page_id: Option<String>, r: &str, diff_only: bool, f: F) -> String
    where
        F: FnOnce(chromiumoxide::page::Page, i64) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<()>>,
    {
        self.act_on_ref_named("action", page_id, r, diff_only, f).await
    }

    async fn act_on_ref_named<F, Fut>(
        &self,
        op_name: &str,
        page_id: Option<String>,
        r: &str,
        diff_only: bool,
        f: F,
    ) -> String
    where
        F: FnOnce(chromiumoxide::page::Page, i64) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<()>>,
    {
        let r_for_page = r.to_string();
        let r_for_ledger = r.to_string();
        let op_name = op_name.to_string();
        let project_key = self.resolve_project_key(None).await;
        match self.session_for(None).await {
            Ok(session) => {
                let result = session
                    .with_page(page_id.as_deref(), move |page| async move {
                        let backend_id = spectra_cdp::resolve_ref(&page, &r_for_page).await?;
                        f(page.clone(), backend_id).await?;
                        spectra_cdp::snapshot::capture_with_diff(&page, diff_only).await
                    })
                    .await;
                let ledger = spectra_cdp::ledger::ledger_for(&project_key).await;
                match result {
                    Ok(snapshot) => {
                        ledger.lock().await.record_action(op_name, r_for_ledger, true);
                        serde_json::to_string(&snapshot).unwrap_or_default()
                    }
                    Err(e) => {
                        ledger.lock().await.record_error(e.to_string());
                        error_json(&e)
                    }
                }
            }
            Err(e) => error_json(&e),
        }
    }
}

fn error_json(e: &anyhow::Error) -> String {
    let chain: Vec<String> = e.chain().map(|c| c.to_string()).collect();
    serde_json::json!({ "error": chain.join(" <- ") }).to_string()
}

/// Attend que le contenu du DOM se stabilise (deux mesures consécutives de
/// la taille du texte visible donnent le même résultat) plutôt qu'un sleep
/// fixe — s'adapte aux pages qui montent vite (SSR/statique) comme à celles
/// qui prennent du temps (SPA lourde). Borné par `timeout_ms` pour ne jamais
/// bloquer indéfiniment sur une page dont le contenu change en continu.
/// Attend que le contenu du DOM se stabilise (deux mesures consécutives
/// identiques) plutôt qu'un sleep fixe. Poll borné côté Rust avec un budget
/// temps strict via `tokio::time::timeout` sur CHAQUE appel `evaluate` — une
/// première version utilisant un `MutationObserver` + `awaitPromise: true`
/// côté page s'est bloquée indéfiniment en pratique (>2 minutes observées),
/// probablement une interaction mal comprise entre le timeout CDP et le
/// polling interne de chromiumoxide ; ce risque de blocage total du serveur
/// est inacceptable, donc abandonné au profit de cette version plus simple
/// et dont chaque round-trip est individuellement borné.
async fn wait_dom_settled(page: &chromiumoxide::page::Page, timeout_ms: u64) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    // 40ms plutôt que les 80ms de la V8 (elle-même déjà réduite depuis 150ms
    // d'origine) : même principe, granularité encore plus fine sur la "queue"
    // de latence après stabilisation réelle du DOM. `stable_streak >= 2`
    // reste le garde-fou inchangé contre un DOM encore en mouvement — un
    // intervalle plus court ne change que la vitesse à laquelle on détecte la
    // stabilité, jamais le critère de stabilité lui-même.
    let poll_interval = std::time::Duration::from_millis(40);
    let eval_timeout = std::time::Duration::from_millis(800);
    let mut last_len: Option<i64> = None;
    let mut stable_streak = 0u32;

    while tokio::time::Instant::now() < deadline {
        let eval = tokio::time::timeout(
            eval_timeout,
            page.evaluate("document.body ? document.body.innerText.length : 0"),
        )
        .await;

        let len = match eval {
            Ok(Ok(r)) => r.value().cloned().and_then(|v| v.as_i64()).unwrap_or(0),
            _ => 0,
        };

        if Some(len) == last_len && len > 0 {
            stable_streak += 1;
            if stable_streak >= 2 {
                return;
            }
        } else {
            stable_streak = 0;
        }
        last_len = Some(len);
        tokio::time::sleep(poll_interval).await;
    }
}

/// Équivalent du `networkidle` de Playwright: attend que le nombre de
/// requêtes en cours (vues mais sans statut/échec encore enregistré) reste
/// à zéro pendant `quiet_ms` d'affilée, borné par `timeout_ms` — pour ne
/// jamais bloquer indéfiniment sur un site avec du polling/websocket actif.
async fn wait_network_idle(page: &chromiumoxide::page::Page, quiet_ms: u64, timeout_ms: u64) {
    let Some(buf) = spectra_cdp::observers::network_buffer_for(page).await else { return };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let poll_interval = std::time::Duration::from_millis(100);
    let mut quiet_for = 0u64;

    while tokio::time::Instant::now() < deadline {
        let pending = buf.lock().await.pending_count();
        if pending == 0 {
            quiet_for += poll_interval.as_millis() as u64;
            if quiet_for >= quiet_ms {
                return;
            }
        } else {
            quiet_for = 0;
        }
        tokio::time::sleep(poll_interval).await;
    }
}
