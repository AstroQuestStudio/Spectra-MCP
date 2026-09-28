use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::sync::Mutex as AsyncMutex;

use crate::console::now_ms;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum LedgerEvent {
    Navigate { url: String, ts: u64 },
    Action { op: String, target: String, ok: bool, ts: u64 },
    Error { message: String, ts: u64 },
}

/// Journal passif d'une session: chaque tool pousse un évènement en fin de
/// traitement (coût quasi nul), sans jamais changer son propre comportement
/// ou son format de retour. `browser_report` lit ce journal a posteriori pour
/// compiler une vue d'ensemble — l'agent n'a plus besoin de se souvenir de
/// tout l'historique de la conversation pour savoir "qu'est-ce qui s'est
/// passé dans cette session de test".
#[derive(Debug, Default)]
pub struct SessionLedger {
    events: Vec<LedgerEvent>,
}

pub type SharedLedger = Arc<AsyncMutex<SessionLedger>>;

static REGISTRY: OnceLock<AsyncMutex<HashMap<String, SharedLedger>>> = OnceLock::new();

fn registry() -> &'static AsyncMutex<HashMap<String, SharedLedger>> {
    REGISTRY.get_or_init(|| AsyncMutex::new(HashMap::new()))
}

pub async fn ledger_for(project_key: &str) -> SharedLedger {
    let mut reg = registry().lock().await;
    reg.entry(project_key.to_string())
        .or_insert_with(|| Arc::new(AsyncMutex::new(SessionLedger::default())))
        .clone()
}

impl SessionLedger {
    pub fn record_navigate(&mut self, url: String) {
        self.events.push(LedgerEvent::Navigate { url, ts: now_ms() });
    }

    pub fn record_action(&mut self, op: impl Into<String>, target: impl Into<String>, ok: bool) {
        self.events.push(LedgerEvent::Action { op: op.into(), target: target.into(), ok, ts: now_ms() });
    }

    pub fn record_error(&mut self, message: impl Into<String>) {
        self.events.push(LedgerEvent::Error { message: message.into(), ts: now_ms() });
    }

    pub fn events(&self) -> &[LedgerEvent] {
        &self.events
    }

    pub fn events_since(&self, since_ts: Option<u64>) -> Vec<&LedgerEvent> {
        self.events
            .iter()
            .filter(|e| {
                let ts = match e {
                    LedgerEvent::Navigate { ts, .. } => *ts,
                    LedgerEvent::Action { ts, .. } => *ts,
                    LedgerEvent::Error { ts, .. } => *ts,
                };
                since_ts.is_none_or(|s| ts >= s)
            })
            .collect()
    }
}
