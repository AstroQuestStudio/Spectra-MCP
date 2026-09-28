use crate::params_types::FieldValue;
use anyhow::{Context, Result};
use chromiumoxide::cdp::browser_protocol::dom::{
    BackendNodeId, FocusParams, GetBoxModelParams, ResolveNodeParams,
    ScrollIntoViewIfNeededParams, SetFileInputFilesParams,
};
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    MouseButton,
};
use chromiumoxide::page::Page;
use std::time::Duration;

fn bid(id: i64) -> BackendNodeId {
    BackendNodeId::new(id)
}

async fn center_of(page: &Page, backend_node_id: i64) -> Result<(f64, f64)> {
    page.execute(ScrollIntoViewIfNeededParams::builder().backend_node_id(bid(backend_node_id)).build())
        .await
        .ok();
    let box_model = page
        .execute(GetBoxModelParams::builder().backend_node_id(bid(backend_node_id)).build())
        .await
        .context("élément introuvable ou sans box model (invisible?)")?;
    let quad = box_model.result.model.content.inner();
    let cx = (quad[0] + quad[2] + quad[4] + quad[6]) / 4.0;
    let cy = (quad[1] + quad[3] + quad[5] + quad[7]) / 4.0;
    Ok((cx, cy))
}

pub async fn click_backend_node(page: &Page, backend_node_id: i64) -> Result<()> {
    let (x, y) = center_of(page, backend_node_id).await?;

    // mouseMoved doit être vu par Chrome avant mousePressed (hover states,
    // listeners mouseenter) — reste séquentiel. mousePressed/mouseReleased
    // en revanche n'ont pas besoin d'attendre l'ACK CDP intermédiaire l'un de
    // l'autre : les deux writes partent sur la même connexion WebSocket, qui
    // garantit l'ordre FIFO d'arrivée. On les émet donc en parallèle
    // (join renvoie dès que les deux réponses sont là, mais Chrome les aura
    // déjà traités dans l'ordre d'émission) plutôt qu'en deux round-trips
    // séquentiels — division par ~1.5-2x du nombre de "réveils" de tâche
    // async attendus par clic.
    page.execute(
        DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MouseMoved)
            .x(x)
            .y(y)
            .build()
            .unwrap(),
    )
    .await?;

    let pressed = DispatchMouseEventParams::builder()
        .r#type(DispatchMouseEventType::MousePressed)
        .x(x)
        .y(y)
        .button(MouseButton::Left)
        .click_count(1)
        .build()
        .unwrap();
    let released = DispatchMouseEventParams::builder()
        .r#type(DispatchMouseEventType::MouseReleased)
        .x(x)
        .y(y)
        .button(MouseButton::Left)
        .click_count(1)
        .build()
        .unwrap();

    let (r1, r2) = futures::join!(page.execute(pressed), page.execute(released));
    r1?;
    r2?;
    Ok(())
}

pub async fn hover_backend_node(page: &Page, backend_node_id: i64) -> Result<()> {
    let (x, y) = center_of(page, backend_node_id).await?;
    page.execute(
        DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MouseMoved)
            .x(x)
            .y(y)
            .build()
            .unwrap(),
    )
    .await?;
    Ok(())
}

pub async fn type_into_backend_node(
    page: &Page,
    backend_node_id: i64,
    text: &str,
    submit: bool,
) -> Result<()> {
    page.execute(FocusParams::builder().backend_node_id(bid(backend_node_id)).build()).await?;

    for ch in text.chars() {
        page.execute(
            DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::Char)
                .text(ch.to_string())
                .build()
                .unwrap(),
        )
        .await?;
    }

    if submit {
        page.execute(
            DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::RawKeyDown)
                .windows_virtual_key_code(13)
                .key("Enter")
                .build()
                .unwrap(),
        )
        .await?;
        page.execute(
            DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::KeyUp)
                .windows_virtual_key_code(13)
                .key("Enter")
                .build()
                .unwrap(),
        )
        .await?;
    }
    Ok(())
}

pub async fn select_option_backend_node(page: &Page, backend_node_id: i64, value: &str) -> Result<()> {
    let js = format!(
        "el.value = {}; el.dispatchEvent(new Event('change', {{bubbles:true}}));",
        serde_json::to_string(value)?
    );
    call_on_backend_node(page, backend_node_id, &js).await
}

async fn call_on_backend_node(page: &Page, backend_node_id: i64, js_fn_body: &str) -> Result<()> {
    let resolved = page
        .execute(ResolveNodeParams::builder().backend_node_id(bid(backend_node_id)).build())
        .await?;
    let object_id = resolved
        .result
        .object
        .object_id
        .clone()
        .context("impossible de résoudre l'objet JS pour ce noeud")?;
    page.execute(
        chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams::builder()
            .function_declaration(format!("function() {{ const el = this; {js_fn_body} }}"))
            .object_id(object_id)
            .build()
            .unwrap(),
    )
    .await?;
    Ok(())
}

pub async fn fill_form(page: &Page, fields: &[FieldValue], diff_only: bool) -> Result<serde_json::Value> {
    let mut results = Vec::new();
    for f in fields {
        let backend_id = crate::resolve_ref(page, &f.r#ref).await;
        match backend_id {
            Ok(id) => {
                let r = type_into_backend_node(page, id, &f.value, false).await;
                results.push(serde_json::json!({ "ref": f.r#ref, "ok": r.is_ok(), "error": r.err().map(|e| e.to_string()) }));
            }
            Err(e) => {
                results.push(serde_json::json!({ "ref": f.r#ref, "ok": false, "error": e.to_string() }));
            }
        }
    }
    let snapshot = crate::snapshot::capture_with_diff(page, diff_only).await?;
    Ok(serde_json::json!({ "results": results, "snapshot": snapshot }))
}

pub async fn screenshot(
    page: &Page,
    r#ref: Option<&str>,
    full_page: bool,
    format: &str,
) -> Result<String> {
    use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
    use chromiumoxide::page::ScreenshotParams as CxScreenshotParams;

    let fmt = if format == "webp" {
        CaptureScreenshotFormat::Webp
    } else {
        CaptureScreenshotFormat::Png
    };

    let mut builder = CxScreenshotParams::builder().format(fmt).full_page(full_page);

    if let Some(r) = r#ref {
        let backend_id = crate::resolve_ref(page, r).await?;
        page.execute(ScrollIntoViewIfNeededParams::builder().backend_node_id(bid(backend_id)).build())
            .await
            .ok();
        let box_model = page
            .execute(GetBoxModelParams::builder().backend_node_id(bid(backend_id)).build())
            .await?;
        let quad = box_model.result.model.content.inner();
        let x = quad[0].min(quad[2]).min(quad[4]).min(quad[6]);
        let y = quad[1].min(quad[3]).min(quad[5]).min(quad[7]);
        let w = quad[0].max(quad[2]).max(quad[4]).max(quad[6]) - x;
        let h = quad[1].max(quad[3]).max(quad[5]).max(quad[7]) - y;
        builder = builder.clip(chromiumoxide::cdp::browser_protocol::page::Viewport {
            x,
            y,
            width: w,
            height: h,
            scale: 1.0,
        });
    }

    let bytes = page.screenshot(builder.build()).await?;
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
    Ok(serde_json::json!({ "image_base64": b64, "format": format }).to_string())
}

/// Vérifie que la ref désigne toujours un élément réellement rendu et
/// visible : `resolve_ref` seul ne garantit que la présence de la ref dans le
/// registre en mémoire (issu du dernier snapshot), pas que l'élément existe
/// encore dans le DOM à l'instant présent — un `GetBoxModel` qui réussit est
/// un signal beaucoup plus fort (l'élément a une géométrie concrète, donc il
/// est rendu). Best-effort : toute erreur (ref inconnue, élément détaché,
/// pas encore rendu) est traitée comme "pas encore satisfait", pas comme un
/// échec fatal de l'attente.
async fn ref_is_visible(page: &Page, r: &str) -> bool {
    let Ok(backend_id) = crate::resolve_ref(page, r).await else { return false };
    page.execute(GetBoxModelParams::builder().backend_node_id(bid(backend_id)).build())
        .await
        .is_ok()
}

/// Attend qu'un texte apparaisse dans la page (`text`) et/ou qu'une ref
/// désigne un élément visible (`r#ref`) — si les deux sont fournis, les deux
/// doivent être satisfaits. `r#ref` était documenté ("Attendre que cette ref
/// existe/soit visible") depuis la V1 mais jamais réellement câblé jusqu'ici
/// — ni transmis par `server.rs`, ni même accepté par cette fonction, qui ne
/// prenait que `text` en paramètre. Corrigé en V22.
pub async fn wait_for(page: &Page, text: Option<&str>, r#ref: Option<&str>, timeout_ms: u64) -> Result<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let text_ok = match text {
            Some(t) => page.content().await.unwrap_or_default().contains(t),
            None => true,
        };
        let ref_ok = match r#ref {
            Some(r) => ref_is_visible(page, r).await,
            None => true,
        };
        if text_ok && ref_ok {
            let snapshot = crate::snapshot::capture(page).await?;
            return Ok(serde_json::json!({ "satisfied": true, "snapshot": snapshot }).to_string());
        }
        if tokio::time::Instant::now() >= deadline {
            let snapshot = crate::snapshot::capture(page).await?;
            return Ok(serde_json::json!({ "satisfied": false, "snapshot": snapshot, "timed_out": true }).to_string());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Message d'erreur lisible sur une exception JS levée par `expression`,
/// plutôt que le `Debug` brut de `ExceptionDetails` (bruit technique :
/// `exception_id`, `script_id`, `stack_trace` complète) qui remontait
/// jusqu'ici tel quel côté agent. Même pattern d'extraction que
/// `spawn_exception_listener` (observers.rs), qui gère le cas symétrique
/// (exception non catchée dans la page, pas dans une expression évaluée).
fn format_js_exception(err: &chromiumoxide::error::CdpError) -> Option<String> {
    let chromiumoxide::error::CdpError::JavascriptException(details) = err else {
        return None;
    };
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
    Some(format!("{message}{location}"))
}

pub async fn evaluate(page: &Page, expression: &str) -> Result<String> {
    let result = match page.evaluate(expression).await {
        Ok(r) => r,
        Err(e) => {
            if let Some(msg) = format_js_exception(&e) {
                anyhow::bail!("l'expression a levé une exception: {msg}");
            }
            return Err(e.into());
        }
    };
    let value = result.value().cloned().unwrap_or(serde_json::Value::Null);
    Ok(serde_json::json!({ "value_json": value }).to_string())
}

/// Lit les Core Web Vitals de la page active. LCP et CLS viennent de
/// `window.__spectraVitals`, alimenté en continu par un `PerformanceObserver`
/// posé avant même le premier chargement (voir observers::install_vitals_observer
/// — poser l'observer après coup via `performance.getEntriesByType` ne
/// retourne quasi jamais l'entrée LCP en pratique, elle n'est peuplée que si
/// un observer bufferisé tournait déjà). FCP et TTFB, eux, restent lisibles
/// après coup sans piège: ce sont des entrées `navigation`/`paint` classiques
/// que Chrome conserve dans le buffer de performance par défaut.
pub async fn performance_metrics(page: &Page) -> Result<serde_json::Value> {
    let script = r#"
        (function() {
            const result = {
                lcp_ms: null, cls: 0, fcp_ms: null, ttfb_ms: null,
                lcp_error: null, cls_error: null,
            };
            const vitals = window.__spectraVitals;
            if (vitals) {
                result.lcp_ms = vitals.lcp > 0 ? Math.round(vitals.lcp) : null;
                result.cls = Math.round(vitals.cls * 1000) / 1000;
                result.lcp_error = vitals.lcpError || null;
                result.cls_error = vitals.clsError || null;
            } else {
                result.lcp_error = "observer non installé (page ouverte avant l'activation de Spectra ?)";
            }

            const nav = performance.getEntriesByType('navigation')[0];
            if (nav) result.ttfb_ms = Math.round(nav.responseStart - nav.requestStart);

            const fcpEntry = performance.getEntriesByType('paint').find(e => e.name === 'first-contentful-paint');
            if (fcpEntry) result.fcp_ms = Math.round(fcpEntry.startTime);

            return result;
        })()
    "#;
    let result = page.evaluate(script).await?;
    Ok(result.value().cloned().unwrap_or(serde_json::Value::Null))
}

pub async fn upload_files(page: &Page, backend_node_id: i64, paths: &[String]) -> Result<()> {
    page.execute(
        SetFileInputFilesParams::builder()
            .backend_node_id(bid(backend_node_id))
            .files(paths.to_vec())
            .build()
            .unwrap(),
    )
    .await?;
    Ok(())
}
