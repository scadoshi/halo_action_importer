use chrono::{FixedOffset, NaiveDateTime, TimeZone};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The action's unique ID, the number Halo keeps in the action ID custom field.
///
/// Parsed once from text, trimmed, so ids from files, the cache and the reports
/// compare as numbers: `0042` and `42` are the same action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct ActionId(u64);

impl ActionId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

impl std::str::FromStr for ActionId {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        trimmed
            .parse()
            .map(Self)
            .map_err(|_| anyhow::anyhow!("action id '{trimmed}' is not a whole number"))
    }
}

impl TryFrom<String> for ActionId {
    type Error = anyhow::Error;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl std::fmt::Display for ActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
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
        alias = "REQUESTID",
        alias = "request_id"
    )]
    pub ticket_id: u32,
    #[serde(
        alias = "actionDate",
        alias = "ActionDate",
        alias = "ACTIONDATE",
        alias = "Actiondate",
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
        alias = "cdactionId",
        alias = "actionId",
        alias = "action_id"
    )]
    pub action_id: ActionId,
    #[serde(default = "default_is_import", rename = "_isimport")]
    pub is_import: bool,
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
    use std::fmt;

    use serde::de::{self, Visitor};

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

/// An action as the import endpoint takes it, with the action ID sent under
/// the configured custom field name.
#[derive(Debug, Clone, Copy)]
pub struct ActionPayload<'a> {
    action: &'a ActionObject,
    custom_field: &'a str,
}

impl Serialize for ActionPayload<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;

        let action = self.action;
        map.serialize_entry("__rowNum__", &Option::<u32>::None)?;
        map.serialize_entry("_isimport", &action.is_import)?;

        if let Some(date) = &action.actiondate {
            let arizona_offset = FixedOffset::west_opt(7 * 3600).unwrap();
            let arizona_dt = arizona_offset
                .from_local_datetime(date)
                .earliest()
                .unwrap_or_else(|| arizona_offset.from_utc_datetime(date));
            let utc_dt = arizona_dt.with_timezone(&chrono::Utc);
            let date_str = utc_dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
            map.serialize_entry("datetime", &date_str)?;
        }

        map.serialize_entry("actionwho", &action.actionwho)?;

        let action_id = action.action_id.value();
        map.serialize_entry(self.custom_field, &action_id)?;

        let customfields = vec![serde_json::json!({
            "name": self.custom_field,
            "value": action_id
        })];
        map.serialize_entry("customfields", &customfields)?;

        map.serialize_entry("note", &action.note)?;
        map.serialize_entry("note_html", &action.note)?;
        map.serialize_entry("outcome", &action.outcome)?;
        map.serialize_entry("requestid", &action.ticket_id)?;
        map.serialize_entry("result", &Option::<String>::None)?;
        map.serialize_entry("ticket_id", &action.ticket_id)?;
        map.serialize_entry("who", &action.actionwho)?;

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
            is_import: true,
        }
    }

    pub fn action_id(&self) -> ActionId {
        self.action_id
    }

    /// The action as the import endpoint takes it, with the ID under `custom_field`.
    pub fn payload<'a>(&'a self, custom_field: &'a str) -> ActionPayload<'a> {
        ActionPayload {
            action: self,
            custom_field,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    #[test]
    fn serializes_to_the_shape_the_import_endpoint_takes() {
        let action_object =
            ActionObject::new(123, None, None, "testing..", "tester", ActionId::new(456));
        let serialized: serde_json::Value =
            serde_json::to_value(action_object.payload("cfactionid")).unwrap();
        assert_eq!(
            serialized,
            serde_json::json!({
                "__rowNum__": null,
                "_isimport": true,
                "actionwho": "tester",
                "cfactionid": 456,
                "customfields": [{ "name": "cfactionid", "value": 456 }],
                "note": "testing..",
                "note_html": "testing..",
                "outcome": "Imported Note",
                "requestid": 123,
                "result": null,
                "ticket_id": 123,
                "who": "tester",
            })
        );
    }

    #[test]
    fn an_action_date_is_read_as_arizona_time_and_sent_as_utc() {
        let local = NaiveDate::from_ymd_opt(2026, 1, 15)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();
        let action_object = ActionObject::new(1, Some(local), None, "n", "w", ActionId::new(1));
        let serialized: serde_json::Value =
            serde_json::to_value(action_object.payload("cfactionid")).unwrap();
        assert_eq!(serialized["datetime"], "2026-01-15T17:00:00.000Z");
    }

    #[test]
    fn an_action_id_that_is_not_a_number_does_not_deserialize() {
        let row = |id: &str| {
            serde_json::json!({
                "ticket_id": 1, "note": "n", "actionwho": "w", "action_id": id, "actiondate": null
            })
        };
        assert!(serde_json::from_value::<ActionObject>(row("abc")).is_err());
        assert!(serde_json::from_value::<ActionObject>(row("")).is_err());
        assert!(serde_json::from_value::<ActionObject>(row("12")).is_ok());
    }

    #[test]
    fn an_isimport_column_still_reads_into_is_import() {
        let row = serde_json::json!({
            "ticket_id": 1, "note": "n", "actionwho": "w", "action_id": "1",
            "actiondate": null, "_isimport": false
        });
        let action: ActionObject = serde_json::from_value(row).unwrap();
        assert!(!action.is_import);
    }

    #[test]
    fn the_action_id_goes_under_the_configured_custom_field() {
        let action_object = ActionObject::new(1, None, None, "n", "w", ActionId::new(9));
        let serialized: serde_json::Value =
            serde_json::to_value(action_object.payload("cfimportid")).unwrap();
        assert_eq!(serialized["cfimportid"], 9);
        assert_eq!(serialized.get("cfactionid"), None);
        assert_eq!(
            serialized["customfields"],
            serde_json::json!([{ "name": "cfimportid", "value": 9 }])
        );
    }
}
