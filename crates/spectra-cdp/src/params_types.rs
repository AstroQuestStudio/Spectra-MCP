use serde::{Deserialize, Serialize};

/// Représentation neutre (sans dépendre de schemars/rmcp) d'un champ de
/// formulaire à remplir, utilisée par spectra-cdp::actions::fill_form.
/// spectra-tools convertit son propre FormField (avec JsonSchema) vers celui-ci.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FieldValue {
    pub r#ref: String,
    pub value: String,
}
