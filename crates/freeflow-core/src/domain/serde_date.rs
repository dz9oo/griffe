//! Sérialisation **ISO 8601** des dates dans tout ce que le cœur expose en JSON (lot 36).
//!
//! Sans annotation, `time::Date` se sérialise avec serde en `[année, jour de l'année]`
//! (`[2026, 245]`) et `OffsetDateTime` en une forme tout aussi opaque — c'est ce que les
//! sorties `--json`, les actions en attente et la liasse montraient jusqu'ici, et ce que
//! l'audit du 2 septembre 2026 a relevé comme illisible pour un humain comme pour un tableur.
//! Ces deux modules s'appliquent avec `#[serde(with = "…")]` (et leurs sous-modules `option`
//! pour un `Option<_>`) : une date devient `"2026-09-02"`, un instant `"2026-09-02T14:03:00Z"`
//! (RFC 3339).
//!
//! **Lecture tolérante.** La désérialisation accepte *aussi* l'ancienne forme en tableau : les
//! entrées du journal d'audit et les actions en attente écrites avant ce lot (`command_json`)
//! restent rejouables — un `freeflow confirm` sur une action déposée la veille de la mise à
//! jour ne doit pas échouer sur un détail de format.

/// `time::Date` ↔ `"AAAA-MM-JJ"`.
pub mod date {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use time::Date;

    use crate::domain::{format_date, parse_date};

    /// Les deux formes admises en lecture : la chaîne ISO (forme écrite depuis le lot 36) et le
    /// tableau `[année, jour de l'année]` que serde produisait avant.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Iso(String),
        Legacy(i32, u16),
    }

    /// # Errors
    ///
    /// Celles du sérialiseur sous-jacent.
    pub fn serialize<S: Serializer>(date: &Date, serializer: S) -> Result<S::Ok, S::Error> {
        format_date(*date).serialize(serializer)
    }

    /// # Errors
    ///
    /// Si la valeur n'est ni une date ISO `AAAA-MM-JJ` ni un tableau `[année, jour]` valide.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Date, D::Error> {
        match Wire::deserialize(deserializer)? {
            Wire::Iso(s) => {
                parse_date(&s).map_err(|e| D::Error::custom(format!("date invalide « {s} » : {e}")))
            }
            Wire::Legacy(year, ordinal) => Date::from_ordinal_date(year, ordinal)
                .map_err(|e| D::Error::custom(format!("date invalide [{year}, {ordinal}] : {e}"))),
        }
    }

    /// `Option<time::Date>` ↔ `"AAAA-MM-JJ"` ou `null`.
    pub mod option {
        use serde::{Deserialize, Deserializer, Serialize, Serializer};
        use time::Date;

        /// # Errors
        ///
        /// Celles du sérialiseur sous-jacent.
        pub fn serialize<S: Serializer>(
            date: &Option<Date>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            date.map(crate::domain::format_date).serialize(serializer)
        }

        /// # Errors
        ///
        /// Voir [`super::deserialize`].
        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<Date>, D::Error> {
            #[derive(Deserialize)]
            struct Wrapper(#[serde(with = "super")] Date);
            Ok(Option::<Wrapper>::deserialize(deserializer)?.map(|w| w.0))
        }
    }
}

/// `time::OffsetDateTime` ↔ RFC 3339 (`"2026-09-02T14:03:00Z"`).
pub mod datetime {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    /// Les deux formes admises en lecture : RFC 3339 (écrite depuis le lot 36) et la forme
    /// tableau/scalaire que `time` produisait avant — relue par le désérialiseur natif de `time`
    /// en second recours.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Rfc3339(String),
        Legacy(OffsetDateTime),
    }

    /// # Errors
    ///
    /// Celles du sérialiseur sous-jacent.
    pub fn serialize<S: Serializer>(
        instant: &OffsetDateTime,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        instant
            .format(&Rfc3339)
            .map_err(|e| serde::ser::Error::custom(e.to_string()))?
            .serialize(serializer)
    }

    /// # Errors
    ///
    /// Si la valeur n'est ni un instant RFC 3339 ni la forme native de `time`.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<OffsetDateTime, D::Error> {
        match Wire::deserialize(deserializer)? {
            Wire::Rfc3339(s) => OffsetDateTime::parse(&s, &Rfc3339)
                .map_err(|e| D::Error::custom(format!("instant invalide « {s} » : {e}"))),
            Wire::Legacy(instant) => Ok(instant),
        }
    }

    /// `Option<time::OffsetDateTime>` ↔ RFC 3339 ou `null`.
    pub mod option {
        use serde::{Deserialize, Deserializer, Serialize, Serializer};
        use time::OffsetDateTime;

        /// # Errors
        ///
        /// Celles du sérialiseur sous-jacent.
        pub fn serialize<S: Serializer>(
            instant: &Option<OffsetDateTime>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            #[derive(Serialize)]
            struct Wrapper<'a>(#[serde(with = "super")] &'a OffsetDateTime);
            instant.as_ref().map(Wrapper).serialize(serializer)
        }

        /// # Errors
        ///
        /// Voir [`super::deserialize`].
        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<OffsetDateTime>, D::Error> {
            #[derive(Deserialize)]
            struct Wrapper(#[serde(with = "super")] OffsetDateTime);
            Ok(Option::<Wrapper>::deserialize(deserializer)?.map(|w| w.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use time::macros::{date, datetime};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Sample {
        #[serde(with = "super::date")]
        on: time::Date,
        #[serde(with = "super::date::option")]
        maybe_on: Option<time::Date>,
        #[serde(with = "super::datetime")]
        at: time::OffsetDateTime,
        #[serde(with = "super::datetime::option")]
        maybe_at: Option<time::OffsetDateTime>,
    }

    #[test]
    fn dates_are_written_in_iso_8601_and_read_back() {
        let sample = Sample {
            on: date!(2026 - 09 - 02),
            maybe_on: None,
            at: datetime!(2026-09-02 14:03:00 UTC),
            maybe_at: Some(datetime!(2026-09-02 14:03:00 +02:00)),
        };
        let json = serde_json::to_string(&sample).unwrap();
        assert_eq!(
            json,
            r#"{"on":"2026-09-02","maybe_on":null,"at":"2026-09-02T14:03:00Z","maybe_at":"2026-09-02T14:03:00+02:00"}"#
        );
        assert_eq!(serde_json::from_str::<Sample>(&json).unwrap(), sample);
    }

    #[test]
    fn the_legacy_array_form_is_still_read() {
        let legacy = r#"{"on":[2026,245],"maybe_on":[2026,1],"at":[2026,245,14,3,0,0,0,0,0],"maybe_at":null}"#;
        let sample: Sample = serde_json::from_str(legacy).unwrap();
        assert_eq!(sample.on, date!(2026 - 09 - 02));
        assert_eq!(sample.maybe_on, Some(date!(2026 - 01 - 01)));
        assert_eq!(sample.at, datetime!(2026-09-02 14:03:00 UTC));
        assert_eq!(sample.maybe_at, None);
    }

    #[test]
    fn an_unreadable_date_is_an_error_not_a_panic() {
        let bad =
            r#"{"on":"02/09/2026","maybe_on":null,"at":"2026-09-02T14:03:00Z","maybe_at":null}"#;
        let err = serde_json::from_str::<Sample>(bad).unwrap_err().to_string();
        assert!(err.contains("02/09/2026"), "{err}");
    }
}
