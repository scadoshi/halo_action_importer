use crate::config::Config;
use chrono::NaiveDateTime;
use serde::{ser::SerializeSeq, Serialize};

#[derive(Debug, Clone)]
pub struct ActionId(u32);

impl ActionId {
    pub fn value(&self) -> u32 {
        self.0
    }

    pub fn new(id: u32) -> Self {
        Self(id)
    }
}

impl Serialize for ActionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let config = Config::from_env()
            .map_err(|e| serde::ser::Error::custom(format!("env errr: {}", e)))?;
        let object = serde_json::json!({
            "id": config.action_id_custom_field_id,
            "value": self.value().to_string()
        });
        let mut seq = serializer.serialize_seq(Some(1))?;
        seq.serialize_element(&object)?;
        seq.end()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionObject {
    ticket_id: u32,
    actiondate: Option<NaiveDateTime>,
    outcome: String,
    note: String,
    actionwho: String,
    #[serde(rename = "customfields")]
    action_id: ActionId,
    _is_import: bool,
}

impl ActionObject {
    pub fn new(
        ticket_id: u32,
        actiondate: Option<NaiveDateTime>,
        outcome: Option<String>,
        note: impl Into<String>,
        actionwho: impl Into<String>,
        action_id: ActionId,
    ) -> Self {
        let outcome = outcome.unwrap_or("Imported Note".to_string());
        Self {
            ticket_id,
            actiondate,
            outcome,
            note: note.into(),
            actionwho: actionwho.into(),
            action_id,
            _is_import: true,
        }
    }

    pub fn action_id(&self) -> u32 {
        self.action_id.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_action_object() {
        let config = Config::from_env().unwrap();
        let action_object =
            ActionObject::new(123, None, None, "testing..", "tester", ActionId::new(456));
        let serialized: serde_json::Value = serde_json::to_value(&action_object).unwrap();
        assert_eq!(
            serialized,
            serde_json::json!({
                "ticket_id": 123,
                "actiondate": null,
                "note": "testing..",
                "outcome": "Imported Note",
                "actionwho": "tester",
                "customfields": [
                    { "id": config.action_id_custom_field_id,"value": "456" }
                ],
                "_is_import": true,
            })
        );
    }
}
