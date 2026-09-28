use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, Clone, Serialize)]
pub struct RequestRecord {
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub status: Option<u16>,
    pub domain: String,
    pub status_text: Option<String>,
    pub failed_reason: Option<String>,
    /// Connexions longue-durée (WebSocket, EventSource) qui ne se
    /// "terminent" jamais — exclues du calcul de network-idle, sinon un
    /// simple HMR de dev server (Vite/webpack) empêche toute détection
    /// de calme réseau.
    pub long_lived: bool,
    /// Headers HTTP de la requête et de la réponse — documentés depuis
    /// longtemps ("détail complet d'une requête (headers, statut)") mais
    /// jamais réellement capturés jusqu'ici : `Request.headers` et
    /// `Response.headers` (CDP) étaient déjà disponibles dans les événements
    /// écoutés (`observers.rs`), simplement jetés au lieu d'être stockés.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestGroup {
    pub domain: String,
    pub status_bucket: String,
    pub count: u32,
    pub ids: Vec<String>,
}

/// Buffer des requêtes réseau d'une page, groupées par domaine+statut au lieu
/// d'une liste plate — le détail complet (headers/body) n'est récupéré que sur
/// demande explicite via request_id, pour ne pas gonfler le contexte par défaut.
/// Capacité bornée (ring FIFO) pour ne jamais grossir indéfiniment.
#[derive(Debug, Default)]
pub struct NetworkBuffer {
    records: HashMap<String, RequestRecord>,
    order: Vec<String>,
    max_entries: usize,
}

pub type SharedNetworkBuffer = Arc<AsyncMutex<NetworkBuffer>>;

pub fn new_shared() -> SharedNetworkBuffer {
    Arc::new(AsyncMutex::new(NetworkBuffer::with_capacity(500)))
}

impl NetworkBuffer {
    pub fn with_capacity(max_entries: usize) -> Self {
        Self { records: HashMap::new(), order: Vec::new(), max_entries }
    }

    pub fn record_request(
        &mut self,
        request_id: String,
        url: String,
        method: String,
        long_lived: bool,
        request_headers: serde_json::Value,
    ) {
        let domain = extract_domain(&url);
        if self.order.len() >= self.max_entries.max(1) && !self.records.contains_key(&request_id) {
            let oldest = self.order.remove(0);
            self.records.remove(&oldest);
        }
        self.records.insert(
            request_id.clone(),
            RequestRecord {
                request_id: request_id.clone(),
                url,
                method,
                status: None,
                domain,
                status_text: None,
                failed_reason: None,
                long_lived,
                request_headers: Some(request_headers),
                response_headers: None,
            },
        );
        self.order.push(request_id);
    }

    /// Nombre de requêtes encore "en vol" (pas de statut/échec), en excluant
    /// les connexions longue-durée (WebSocket/EventSource) qui ne se
    /// terminent jamais — c'est ce que `wait_network_idle` interroge.
    pub fn pending_count(&self) -> u32 {
        self.records
            .values()
            .filter(|r| !r.long_lived && r.status.is_none() && r.failed_reason.is_none())
            .count() as u32
    }

    pub fn record_response(
        &mut self,
        request_id: &str,
        status: u16,
        status_text: String,
        response_headers: serde_json::Value,
    ) {
        if let Some(r) = self.records.get_mut(request_id) {
            r.status = Some(status);
            r.status_text = Some(status_text);
            r.response_headers = Some(response_headers);
        }
    }

    pub fn record_failure(&mut self, request_id: &str, reason: String) {
        if let Some(r) = self.records.get_mut(request_id) {
            r.failed_reason = Some(reason);
        }
    }

    pub fn get(&self, request_id: &str) -> Option<&RequestRecord> {
        self.records.get(request_id)
    }

    pub fn grouped(&self, status_filter: StatusFilter) -> Vec<RequestGroup> {
        let mut groups: HashMap<(String, String), Vec<String>> = HashMap::new();
        for id in &self.order {
            let Some(r) = self.records.get(id) else { continue };
            let bucket = status_bucket(r.status);
            if !status_filter.matches(r.status) {
                continue;
            }
            groups
                .entry((r.domain.clone(), bucket))
                .or_default()
                .push(r.request_id.clone());
        }
        groups
            .into_iter()
            .map(|((domain, status_bucket), ids)| RequestGroup {
                domain,
                status_bucket,
                count: ids.len() as u32,
                ids,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum StatusFilter {
    All,
    ClientError,
    ServerError,
}

impl StatusFilter {
    pub fn parse(s: Option<&str>) -> Self {
        match s {
            Some("4xx") => StatusFilter::ClientError,
            Some("5xx") => StatusFilter::ServerError,
            _ => StatusFilter::All,
        }
    }

    fn matches(&self, status: Option<u16>) -> bool {
        match (self, status) {
            (StatusFilter::All, _) => true,
            (StatusFilter::ClientError, Some(s)) => (400..500).contains(&s),
            (StatusFilter::ServerError, Some(s)) => (500..600).contains(&s),
            _ => false,
        }
    }
}

fn status_bucket(status: Option<u16>) -> String {
    match status {
        None => "pending".to_string(),
        Some(s) if s < 300 => "2xx".to_string(),
        Some(s) if s < 400 => "3xx".to_string(),
        Some(s) if s < 500 => "4xx".to_string(),
        Some(_) => "5xx".to_string(),
    }
}

fn extract_domain(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .to_string()
}
