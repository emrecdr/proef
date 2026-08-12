//! JSON Schema for packs: schemars-derived from the serde model **plus** the
//! step-kind fragments contributed by registered engines (TECH-SPEC §6).

use crate::engine::StepKindSpec;

/// The pack JSON Schema with engine payload fragments merged into the step
/// object's properties. Falls back to the plain derived schema when the
/// generated shape does not match expectations (never fails).
pub fn json_schema(kinds: &[StepKindSpec]) -> serde_json::Value {
    let schema = schemars::schema_for!(super::RawPack);
    let mut root = serde_json::to_value(&schema).unwrap_or(serde_json::Value::Bool(true));

    if let Some(step_properties) = root
        .pointer_mut("/$defs/RawStep/properties")
        .and_then(serde_json::Value::as_object_mut)
    {
        for kind in kinds {
            let fragment: serde_json::Value =
                serde_json::from_str(kind.schema).unwrap_or(serde_json::Value::Bool(true));
            step_properties.insert(kind.prefix.to_owned(), fragment);
        }
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_fragments_merge_into_the_step_schema() {
        let kinds = [StepKindSpec {
            prefix: "hurl",
            schema: r#"{ "type": "string" }"#,
            validate: None,
            fragments: None,
            options: None,
        }];
        let schema = json_schema(&kinds);
        assert_eq!(
            schema.pointer("/$defs/RawStep/properties/hurl/type"),
            Some(&serde_json::Value::String("string".into()))
        );
        // The fixed schema part is still present.
        assert!(schema.pointer("/$defs/RawMacro/properties/match").is_some());
    }
}
