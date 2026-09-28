use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LaunchParams {
    #[schemars(description = "Répertoire absolu du projet courant (sert de clé de session Chrome). Par défaut, le cwd du serveur.")]
    pub project: Option<String>,
    #[schemars(description = "\"attach\" pour rattacher un Chrome existant uniquement, \"spawn\" pour forcer un nouveau, \"auto\" (défaut) pour essayer attach puis spawn")]
    pub mode: Option<String>,
    #[schemars(description = "Lancer Chrome en headless (défaut: false, fenêtre visible)")]
    pub headless: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PageScopedParams {
    #[schemars(description = "Identifiant de page/onglet ciblé (voir browser_tabs). Défaut: page active.")]
    pub page_id: Option<String>,
    #[schemars(description = "Si true et qu'un snapshot précédent existe pour cette page, retourne un résumé compact du changement (lignes ajoutées/retirées) plutôt que l'arbre complet. Défaut: false.")]
    pub diff_only: Option<bool>,
    #[schemars(description = "Si true, ajoute une liste \"a11y_issues\" de problèmes d'accessibilité basiques détectés (nom accessible manquant sur élément interactif, image sans alt, rôles ARIA imbriqués de façon incohérente). Détection volontairement simple, pas un remplacement d'axe-core/Lighthouse. Défaut: false.")]
    pub check_a11y: Option<bool>,
    #[schemars(description = "Niveau de détail du texte retourné, pour contrôler le coût en tokens : \"full\" (défaut) l'arbre complet avec la hiérarchie structurelle ; \"compact\" ne garde que les éléments actionnables/nommés (retire les nœuds structurels purs comme main/banner/paragraph sans nom) ; \"refs_only\" une seule ligne par ref sans indentation ni hiérarchie, le minimum pour cibler un clic. Sans effet si diff_only=true (déjà minimal).")]
    pub detail: Option<String>,
    #[schemars(description = "Ref d'un snapshot précédent (ex: \"e12\") pour ne capturer QUE le sous-arbre enraciné à cet élément (ex: une modale ouverte) au lieu de la page entière. Réduit fortement le texte retourné sur les pages denses. Si la ref ne correspond plus à aucun nœud du DOM actuel (élément disparu/recréé), retourne un message explicite plutôt qu'un arbre vide silencieux.")]
    pub root_ref: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NavigateParams {
    #[schemars(description = "URL de destination (http/https)")]
    pub url: String,
    #[schemars(description = "\"load\" (défaut) ou \"networkidle\"")]
    pub wait_until: Option<String>,
    pub page_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RefParams {
    #[schemars(description = "Référence d'élément issue d'un browser_snapshot précédent (ex: \"e12\")")]
    pub r#ref: String,
    pub page_id: Option<String>,
    #[schemars(description = "Si true, retourne un résumé compact des nœuds ajoutés\\retirés\\modifiés depuis le snapshot précédent de cette page, plutôt que l'arbre complet. Défaut: false.")]
    pub diff_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TypeParams {
    #[schemars(description = "Référence d'élément issue d'un browser_snapshot précédent")]
    pub r#ref: String,
    #[schemars(description = "Texte à saisir")]
    pub text: String,
    #[schemars(description = "Appuyer sur Entrée après la saisie (défaut: false)")]
    pub submit: Option<bool>,
    pub page_id: Option<String>,
    #[schemars(description = "Si true, retourne un résumé compact des nœuds ajoutés\\retirés\\modifiés depuis le snapshot précédent de cette page, plutôt que l'arbre complet. Défaut: false.")]
    pub diff_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FormField {
    pub r#ref: String,
    pub value: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FillFormParams {
    #[schemars(description = "Liste de champs à remplir en un seul appel")]
    pub fields: Vec<FormField>,
    pub page_id: Option<String>,
    #[schemars(description = "Si true, retourne un résumé compact des nœuds ajoutés\\retirés\\modifiés depuis le snapshot précédent de cette page, plutôt que l'arbre complet. Défaut: false.")]
    pub diff_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SelectOptionParams {
    pub r#ref: String,
    #[schemars(description = "Valeur de l'option à sélectionner")]
    pub value: String,
    pub page_id: Option<String>,
    #[schemars(description = "Si true, retourne un résumé compact des nœuds ajoutés\\retirés\\modifiés depuis le snapshot précédent de cette page, plutôt que l'arbre complet. Défaut: false.")]
    pub diff_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScreenshotParams {
    #[schemars(description = "Référence d'un élément précis à capturer (sinon page entière ou viewport)")]
    pub r#ref: Option<String>,
    #[schemars(description = "Capturer la page entière (scroll complet), défaut: false (viewport visible)")]
    pub full_page: Option<bool>,
    #[schemars(description = "\"png\" (défaut) ou \"webp\"")]
    pub format: Option<String>,
    pub page_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaitForParams {
    #[schemars(description = "Attendre que ce texte apparaisse dans la page")]
    pub text: Option<String>,
    #[schemars(description = "Attendre que cette ref existe/soit visible")]
    pub r#ref: Option<String>,
    #[schemars(description = "Délai maximum en millisecondes (défaut: 5000)")]
    pub timeout_ms: Option<u64>,
    pub page_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConsoleMessagesParams {
    #[schemars(description = "\"error\", \"warning\", ou \"all\" (défaut)")]
    pub level: Option<String>,
    #[schemars(description = "Ne retourner que les messages vus après ce timestamp (ms epoch)")]
    pub since_ts: Option<u64>,
    pub page_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NetworkRequestsParams {
    #[schemars(description = "\"4xx\", \"5xx\", ou \"all\" (défaut)")]
    pub status_filter: Option<String>,
    pub page_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NetworkRequestDetailParams {
    #[schemars(description = "Identifiant de requête retourné par browser_network_requests")]
    pub request_id: String,
    pub page_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EvaluateParams {
    #[schemars(description = "Expression JavaScript à exécuter dans le contexte de la page")]
    pub expression: String,
    pub page_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PerformanceMetricsParams {
    #[schemars(description = "Identifiant de page\\onglet ciblé (voir browser_tabs). Défaut: page active.")]
    pub page_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TabsParams {
    #[schemars(description = "\"list\", \"new\", \"close\", ou \"select\"")]
    pub action: String,
    pub page_id: Option<String>,
    #[schemars(description = "URL pour action=\"new\"")]
    pub url: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileUploadParams {
    pub r#ref: String,
    #[schemars(description = "Chemins absolus des fichiers à uploader")]
    pub paths: Vec<String>,
    pub page_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunRecipeParams {
    #[schemars(description = "Nom du projet (dossier contenant .spectra/recipes/)")]
    pub project: String,
    #[schemars(description = "Nom de la recipe, sans extension .mjs")]
    pub recipe: String,
    #[schemars(description = "Arguments JSON passés à la recipe")]
    pub args: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionsParams {
    #[schemars(description = "\"list\" (défaut) pour voir les sessions actives, \"close\" pour en fermer une, \"close_all\" pour tout fermer d'un coup (arrête tous les Chrome spawnés par ce serveur)")]
    pub action: String,
    #[schemars(description = "Clé de projet à fermer (requis si action=\"close\")")]
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReportParams {
    #[schemars(description = "Ne compiler que les évènements depuis ce timestamp ms epoch (défaut: depuis le début de la session)")]
    pub since_ts: Option<u64>,
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SequenceAction {
    #[schemars(description = "\"click\", \"type\", \"select_option\"")]
    pub op: String,
    #[schemars(description = "Référence d'élément issue d'un browser_snapshot précédent")]
    pub r#ref: String,
    #[schemars(description = "Valeur pour \"type\"/\"select_option\" (ignoré pour \"click\")")]
    pub value: Option<String>,
    #[schemars(description = "\"abort_sequence\" (défaut) ou \"skip_and_continue\" en cas d'échec de cette étape")]
    pub on_error: Option<String>,
    #[schemars(description = "Étiquette libre pour identifier l'étape dans le rapport")]
    pub label: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ActSequenceParams {
    #[schemars(description = "Liste d'actions à exécuter séquentiellement sans repasser par un tour de raisonnement entre chaque étape")]
    pub actions: Vec<SequenceAction>,
    pub page_id: Option<String>,
    #[schemars(description = "Si true, ajoute au résultat un résumé compact (\"snapshot\") des nœuds ajoutés\\retirés\\modifiés depuis le snapshot précédent de cette page, capturé une seule fois après la dernière étape exécutée (pas par étape). Défaut: false (aucun snapshot retourné, comportement historique).")]
    pub diff_only: Option<bool>,
}
