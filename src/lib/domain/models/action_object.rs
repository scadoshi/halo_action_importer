use crate::config::Config;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize, ser::SerializeSeq};

#[derive(Debug, Clone)]
pub struct ActionId(String);

impl ActionId {
    pub fn value(&self) -> &str {
        &self.0
    }

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl Serialize for ActionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let config = Config::from_env()
            .map_err(|e| serde::ser::Error::custom(format!("env error: {}", e)))?;
        let object = serde_json::json!({
            "id": config.action_id_custom_field_id,
            "value": self.value()
        });
        let mut seq = serializer.serialize_seq(Some(1))?;
        seq.serialize_element(&object)?;
        seq.end()
    }
}

impl<'de> Deserialize<'de> for ActionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self(s))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionObject {
    #[serde(
        alias = "requestId",
        alias = "requestID",
        alias = "RequestId",
        alias = "RequestID",
        alias = "requestid",
        alias = "REQUESTID"
    )]
    ticket_id: u32,
    #[serde(alias = "actionDate", alias = "ActionDate", alias = "ACTIONDATE")]
    actiondate: Option<NaiveDateTime>,
    #[serde(default = "default_outcome")]
    outcome: String,
    #[serde(alias = "Note", alias = "NOTE")]
    note: String,
    #[serde(alias = "actionWho", alias = "ActionWho")]
    actionwho: String,
    #[serde(rename(serialize = "customfields"))]
    #[serde(
        alias = "cfactionid",
        alias = "CFACTIONID",
        alias = "CfActionId",
        alias = "cfActionId",
        alias = "CFactionId",
        alias = "CFActionID",
        alias = "CFactionId",
        alias = "cfactionId",
        alias = "cfActionID",
        alias = "cfactionID",
        alias = "cdactionId"
    )]
    action_id: ActionId,
    #[serde(default = "default_is_import")]
    _is_import: bool,
}

fn default_outcome() -> String {
    "Imported Note".to_string()
}

fn default_is_import() -> bool {
    true
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
        let outcome = outcome.unwrap_or_else(|| default_outcome());
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

    pub fn action_id(&self) -> &str {
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
            ActionObject::new(123, None, None, "testing..", "tester", ActionId::new("456"));
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
