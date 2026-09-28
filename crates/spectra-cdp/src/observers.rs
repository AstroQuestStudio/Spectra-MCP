use crate::console::{self, SharedConsoleBuffer};
use crate::network::{self, SharedNetworkBuffer};
use chromiumoxide::cdp::browser_protocol::network::{
    EnableParams as NetworkEnableParams, EventLoadingFailed, EventRequestWillBeSent,
    EventResponseReceived,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    EnableParams as RuntimeEnableParams, EventConsoleApiCalled, EventExceptionThrown,
};
use chromiumoxide::page::Page;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::sync::Mutex as AsyncMutex;

struct PageObservers {
    console: SharedConsoleBuffer,
    network: SharedNetworkBuffer,
}

static REGISTRY: OnceLock<AsyncMutex<HashMap<String, PageObservers>>> = OnceLock::new();

fn registry() -> &'static AsyncMutex<HashMap<String, PageObservers>> {
    REGISTRY.get_or_init(|| AsyncMutex::new(HashMap::new()))
}

/// Active `Runtime.enable` + `Network.enable` sur la page et démarre les
/// tâches de fond qui accumulent les messages console et requêtes réseau dans
/// des buffers dédiés (dédupliqués, capacité bornée). Idempotent : n'active
/// qu'une fois par page (target_id), les appels suivants sont des no-ops.
pub async fn ensure_observing(page: &Page) -> anyhow::Result<()> {
    let target = page.target_id().inner().to_string();
    {
        let reg = registry().lock().await;
        if reg.contains_key(&target) {
            return Ok(());
        }
    }

    let console_buf = console::new_shared();
    let network_buf = network::new_shared();

    page.execute(RuntimeEnableParams::default()).await.ok();
    page.execute(NetworkEnableParams::default()).await.ok();

    spawn_console_listener(page, console_buf.clone()).await;
    spawn_exception_listener(page, console_buf.clone()).await;
    spawn_network_listeners(page, network_buf.clone()).await;
    install_vitals_observer(page).await;

    let mut reg = registry().lock().await;
    reg.insert(target, PageObservers { console: console_buf, network: network_buf });
    Ok(())
}

pub async fn console_buffer_for(page: &Page) -> Option<SharedConsoleBuffer> {
    let target = page.target_id().inner().to_string();
    registry().lock().await.get(&target).map(|o| o.console.clone())
}

pub async fn network_buffer_for(page: &Page) -> Option<SharedNetworkBuffer> {
    let target = page.target_id().inner().to_string();
    registry().lock().await.get(&target).map(|o| o.network.clone())
}

async fn spawn_console_listener(page: &Page, buf: SharedConsoleBuffer) {
    if let Ok(mut stream) = page.event_listener::<EventConsoleApiCalled>().await {
        tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                let kind = format!("{:?}", event.r#type).to_lowercase();
                let text = event
                    .args
                    .iter()
                    .filter_map(|a| {
                        a.value
                            .as_ref()
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                            .or_else(|| a.description.clone())
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                buf.lock().await.push(&kind, text);
            }
        });
    }
}

/// Capture les exceptions JS non catchées et rejections de promesses non
/// gérées (CDP les signale toutes deux via `Runtime.exceptionThrown`, sans
/// distinction d'événement — le texte contient "Uncaught (in promise)" pour
/// le second cas). Poussées dans le même buffer console au niveau "error",
/// avec la localisation (fichier:ligne:colonne) pour rester actionnable sans
/// tool dédié ni changement de format de retour pour l'agent.
async fn spawn_exception_listener(page: &Page, buf: SharedConsoleBuffer) {
    if let Ok(mut stream) = page.event_listener::<EventExceptionThrown>().await {
        tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                let details = &event.exception_details;
                let message = details
                    .exception
                    .as_ref()
                    .and_then(|e| e.description.clone())
                    .unwrap_or_else(|| details.text.clone());
                let location = details
                    .url
                    .as_ref()
                    .map(|u| format!(" ({u}:{}:{})", details.line_number, details.column_number))
                    .unwrap_or_default();
                buf.lock().await.push("error", format!("{message}{location}"));
            }
        });
    }
}

/// Script posé sur `Page.addScriptToEvaluateOnNewDocument` (via
/// `evaluate_on_new_document`) pour que les observers tournent dès le tout
/// début du chargement de la page, avant qu'on puisse `evaluate()` quoi que
/// ce soit depuis Rust — condition nécessaire pour capturer LCP (qui se fige
/// dès la première interaction/changement d'onglet) et les layout shifts
/// (qui peuvent survenir dès les premiers ms de rendu).
///
/// Piège trouvé par diagnostic isolé : poser l'observer `layout-shift` avec
/// `buffered: true` AVANT la navigation bloque indéfiniment `Page.goto` sur
/// ce Chrome (le `PerformanceObserver` semble interférer avec le lifecycle
/// `Page.frameNavigated`/`Page.loadEventFired` quand il tente de rejouer un
/// buffer vide au tout premier rendu). `buffered: true` n'apporte de toute
/// façon rien ici : rien n'a encore pu être émis avant l'appel `observe()`
/// puisque le script s'exécute avant tout chargement. Seul l'observer LCP
/// garde `buffered: true`, car il doit pouvoir rattraper une entrée émise
/// entre le moment où Chrome charge le document et celui où ce script
/// s'exécute réellement (l'ordre exact des deux n'est pas garanti par CDP).
const VITALS_SCRIPT: &str = r#"
    window.__spectraVitals = { lcp: 0, cls: 0 };
    try {
        new PerformanceObserver((list) => {
            const entries = list.getEntries();
            const last = entries[entries.length - 1];
            if (last) window.__spectraVitals.lcp = last.startTime;
        }).observe({ type: 'largest-contentful-paint', buffered: true });
    } catch (e) { window.__spectraVitals.lcpError = String(e); }
    try {
        new PerformanceObserver((list) => {
            for (const entry of list.getEntries()) {
                if (!entry.hadRecentInput) window.__spectraVitals.cls += entry.value;
            }
        }).observe({ type: 'layout-shift', buffered: false });
    } catch (e) { window.__spectraVitals.clsError = String(e); }
"#;

async fn install_vitals_observer(page: &Page) {
    page.evaluate_on_new_document(VITALS_SCRIPT).await.ok();
}

async fn spawn_network_listeners(page: &Page, buf: SharedNetworkBuffer) {
    if let Ok(mut stream) = page.event_listener::<EventRequestWillBeSent>().await {
        let buf = buf.clone();
        tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                let long_lived = matches!(
                    event.r#type,
                    Some(chromiumoxide::cdp::browser_protocol::network::ResourceType::WebSocket)
                        | Some(chromiumoxide::cdp::browser_protocol::network::ResourceType::EventSource)
                );
                buf.lock().await.record_request(
                    event.request_id.inner().to_string(),
                    event.request.url.clone(),
                    event.request.method.clone(),
                    long_lived,
                    event.request.headers.inner().clone(),
                );
            }
        });
    }
    if let Ok(mut stream) = page.event_listener::<EventResponseReceived>().await {
        let buf = buf.clone();
        tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                buf.lock().await.record_response(
                    &event.request_id.inner().to_string(),
                    event.response.status as u16,
                    event.response.status_text.clone(),
                    event.response.headers.inner().clone(),
                );
            }
        });
    }
    if let Ok(mut stream) = page.event_listener::<EventLoadingFailed>().await {
        tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                buf.lock()
                    .await
                    .record_failure(&event.request_id.inner().to_string(), event.error_text.clone());
            }
        });
    }
}
