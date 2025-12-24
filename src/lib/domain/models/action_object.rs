use chrono::{FixedOffset, NaiveDateTime, TimeZone};
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeSeq};

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
        let object = serde_json::json!({
            "name": "cfactionid",
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

#[derive(Debug, Clone, Deserialize)]
pub struct ActionObject {
    #[serde(
        alias = "requestId",
        alias = "requestID",
        alias = "RequestId",
        alias = "RequestID",
        alias = "requestid",
        alias = "REQUESTID"
    )]
    pub ticket_id: u32,
    #[serde(
        alias = "actionDate",
        alias = "ActionDate",
        alias = "ACTIONDATE",
        deserialize_with = "deserialize_action_date",
        skip_serializing_if = "Option::is_none"
    )]
    pub actiondate: Option<NaiveDateTime>,
    #[serde(default = "default_outcome")]
    pub outcome: String,
    #[serde(alias = "Note", alias = "NOTE")]
    pub note: String,
    #[serde(alias = "actionWho", alias = "ActionWho")]
    pub actionwho: String,
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
    pub action_id: ActionId,
    #[serde(default = "default_is_import", rename = "_isimport")]
    pub _isimport: bool,
}

fn default_outcome() -> String {
    "Imported Note".to_string()
}

fn default_is_import() -> bool {
    true
}

fn deserialize_action_date<'de, D>(deserializer: D) -> Result<Option<NaiveDateTime>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct ActionDateVisitor;

    impl<'de> Visitor<'de> for ActionDateVisitor {
        type Value = Option<NaiveDateTime>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an optional date string in ISO 8601 format")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_str(self)
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value.trim().is_empty() {
                return Ok(None);
            }
            let cleaned = value.trim().trim_end_matches('Z').trim_end_matches('z');
            NaiveDateTime::parse_from_str(cleaned, "%Y-%m-%dT%H:%M:%S%.f")
                .or_else(|_| NaiveDateTime::parse_from_str(cleaned, "%Y-%m-%dT%H:%M:%S"))
                .or_else(|_| NaiveDateTime::parse_from_str(cleaned, "%Y-%m-%d %H:%M:%S"))
                .or_else(|_| NaiveDateTime::parse_from_str(cleaned, "%Y-%m-%dT%H:%M:%S%.fZ"))
                .or_else(|_| NaiveDateTime::parse_from_str(cleaned, "%Y-%m-%dT%H:%M:%SZ"))
                .map(Some)
                .map_err(|e| de::Error::custom(format!("failed to parse date '{}': {}", value, e)))
        }
    }

    deserializer.deserialize_option(ActionDateVisitor)
}

impl Serialize for ActionObject {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;

        map.serialize_entry("__rowNum__", &Option::<u32>::None)?;
        map.serialize_entry("_isimport", &self._isimport)?;

        if let Some(date) = &self.actiondate {
            let arizona_offset = FixedOffset::west_opt(7 * 3600).unwrap();
            let arizona_dt = arizona_offset
                .from_local_datetime(date)
                .earliest()
                .unwrap_or_else(|| arizona_offset.from_utc_datetime(date));
            let utc_dt = arizona_dt.with_timezone(&chrono::Utc);
            let date_str = utc_dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
            map.serialize_entry("datetime", &date_str)?;
        }

        map.serialize_entry("actionwho", &self.actionwho)?;

        let cfactionid: u32 = self.action_id.value().parse().unwrap_or(0);
        map.serialize_entry("cfactionid", &cfactionid)?;

        let customfields = vec![serde_json::json!({
            "name": "cfactionid",
            "value": cfactionid
        })];
        map.serialize_entry("customfields", &customfields)?;

        map.serialize_entry("note", &self.note)?;
        map.serialize_entry("note_html", &self.note)?;
        map.serialize_entry("outcome", &self.outcome)?;
        map.serialize_entry("requestid", &self.ticket_id)?;
        map.serialize_entry("result", &Option::<String>::None)?;
        map.serialize_entry("ticket_id", &self.ticket_id)?;
        map.serialize_entry("who", &self.actionwho)?;

        map.end()
    }
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
        let outcome = outcome.unwrap_or_else(default_outcome);
        Self {
            ticket_id,
            actiondate,
            outcome,
            note: note.into(),
            actionwho: actionwho.into(),
            action_id,
            _isimport: true,
        }
    }

    pub fn action_id(&self) -> &str {
        self.action_id.value()
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

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
                "_isimport": true,
            })
        );
    }
}
