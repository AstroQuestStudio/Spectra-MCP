use rmcp::transport::stdio;
use rmcp::ServiceExt;
use spectra_tools::SpectraTools;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout est réservé au protocole MCP (JSON-RPC) — tous les logs vont sur stderr.
    //
    // `chromiumoxide` loggue en ERROR chaque message CDP qu'il échoue à
    // désérialiser — en pratique, presque exclusivement
    // Network.requestWillBeSentExtraInfo/responseReceivedExtraInfo, dont un
    // champ (`resourceIPAddressSpace: "Loopback"`, une valeur d'enum ajoutée
    // par Chrome mais absente de la version d'enum couverte par
    // `chromiumoxide_cdp 0.7.0`) fait échouer toute la désérialisation du
    // message. Sur une session de navigation normale, ça représentait ~99.8%
    // du volume de logs (6929 lignes sur 6945 mesurées sur une seule
    // navigation). Ce n'est pas fatal : `Handler` avale l'erreur et continue
    // sa boucle d'événements normalement (vérifié en lisant son code), donc
    // aucune fonctionnalité n'est perdue — seul le bruit l'est, ici.
    let default_filter = "info,chromiumoxide::conn=off,chromiumoxide::handler=off";
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)))
        .init();

    tracing::info!("spectra-server démarrage (stdio)");

    let service = SpectraTools::new().serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}
