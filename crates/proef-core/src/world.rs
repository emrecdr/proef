//! Typed variable scope and persistent global store (ADR-0005).
//!
//! The [`World`] is the interop bus between batches and engines: one typed scenario
//! scope plus the persistent [`GlobalStore`] (`.proef-state.json`). Values are typed
//! (mirroring hurl's `Value` subset) because captures cross engine boundaries —
//! stringly-typed round-trips would lose numbers and booleans.
//!
//! Persistence note (core purity): [`GlobalStore::load`] / [`GlobalStore::save`] are
//! the crate's single sanctioned IO edge, invoked only by the orchestrating CLI —
//! never from pipeline code. Writes are atomic (sibling temp file + rename) and the
//! state file is created `0600` (TECH-SPEC §11).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// A typed variable value (mirrors the hurl `Value` subset that crosses the seam).
///
/// Serialized as the natural JSON scalar. Deserialization is a hand-written
/// visitor rather than `#[serde(untagged)]`: hurl enables
/// `serde_json/arbitrary_precision` by default, cargo feature-unifies that into
/// every workspace build, and under it numbers reach untagged enums as `serde_json`'s
/// private number token (which no plain `i64`/`f64` variant matches). The visitor
/// accepts both encodings — pinned by `value_json_forms_round_trip`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Value {
    /// Absent/null.
    Null,
    /// Boolean.
    Bool(bool),
    /// Integer number.
    Int(i64),
    /// Floating-point number.
    Float(f64),
    /// String.
    String(String),
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};

        struct ValueVisitor;

        impl<'de> Visitor<'de> for ValueVisitor {
            type Value = Value;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a scalar (null, bool, number, or string)")
            }

            fn visit_unit<E>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }

            fn visit_bool<E>(self, b: bool) -> Result<Value, E> {
                Ok(Value::Bool(b))
            }

            fn visit_i64<E>(self, i: i64) -> Result<Value, E> {
                Ok(Value::Int(i))
            }

            fn visit_u64<E>(self, u: u64) -> Result<Value, E> {
                // Beyond i64 range: keep totality, deliberately degrade to float.
                #[allow(clippy::cast_precision_loss)]
                Ok(i64::try_from(u).map_or(Value::Float(u as f64), Value::Int))
            }

            fn visit_f64<E>(self, f: f64) -> Result<Value, E> {
                Ok(Value::Float(f))
            }

            fn visit_str<E>(self, s: &str) -> Result<Value, E> {
                Ok(Value::String(s.to_owned()))
            }

            fn visit_string<E>(self, s: String) -> Result<Value, E> {
                Ok(Value::String(s))
            }

            // The `arbitrary_precision` path: numbers arrive as a single-entry
            // magic map that only `serde_json::Number` knows how to decode.
            fn visit_map<A>(self, map: A) -> Result<Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let number =
                    serde_json::Number::deserialize(de::value::MapAccessDeserializer::new(map))?;
                if let Some(i) = number.as_i64() {
                    Ok(Value::Int(i))
                } else if let Some(f) = number.as_f64() {
                    Ok(Value::Float(f))
                } else {
                    Err(de::Error::custom(format!(
                        "unrepresentable number {number}"
                    )))
                }
            }
        }

        deserializer.deserialize_any(ValueVisitor)
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Null => f.write_str(""),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Int(i) => write!(f, "{i}"),
            Self::Float(x) => write!(f, "{x}"),
            Self::String(s) => f.write_str(s),
        }
    }
}

/// The persistent global variable store (`.proef-state.json`, ADR-0005).
///
/// `saveAs: global` promotes captures here; values survive across scenarios
/// and runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GlobalStore {
    values: BTreeMap<String, Value>,
}

impl GlobalStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a value.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }

    /// Insert or replace a value.
    pub fn insert(&mut self, name: impl Into<String>, value: Value) {
        self.values.insert(name.into(), value);
    }

    /// Iterate entries in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Load the store from `path`. A missing file yields an empty store; any other
    /// failure is a [`CoreError::System`].
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::new());
            }
            Err(err) => {
                return Err(CoreError::system_with(
                    format!("cannot read global state file {}", path.display()),
                    err,
                ));
            }
        };
        let values: BTreeMap<String, Value> = serde_json::from_str(&text).map_err(|err| {
            CoreError::system_with(
                format!("global state file {} is not valid JSON", path.display()),
                err,
            )
        })?;
        Ok(Self { values })
    }

    /// Persist the store to `path` atomically: write a sibling temp file (mode
    /// `0600` on unix), then rename over the target.
    pub fn save(&self, path: &Path) -> Result<(), CoreError> {
        let json = serde_json::to_string_pretty(&self.values)
            .map_err(|err| CoreError::system_with("cannot serialize global state", err))?;
        let tmp = sibling_tmp_path(path);
        fs::write(&tmp, json).map_err(|err| {
            CoreError::system_with(
                format!("cannot write global state temp file {}", tmp.display()),
                err,
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).map_err(|err| {
                CoreError::system_with(format!("cannot set permissions on {}", tmp.display()), err)
            })?;
        }
        fs::rename(&tmp, path).map_err(|err| {
            CoreError::system_with(
                format!("cannot move global state into place at {}", path.display()),
                err,
            )
        })
    }
}

/// `<path>.tmp` next to the target, so the final rename stays on one filesystem.
fn sibling_tmp_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    // Process-unique: concurrent proef processes (the harness) must not
    // truncate each other's in-flight temp file.
    os.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(os)
}

/// One typed variable scope per scenario, layered over the persistent global store.
///
/// Reads resolve scenario-first (scenario values shadow globals); `saveAs: global`
/// writes through to the store.
#[derive(Debug, Default)]
pub struct World {
    scenario: BTreeMap<String, Value>,
    global: GlobalStore,
    /// Keys written through [`World::set_global`] during this scenario — the
    /// *write set* merged back into the shared store. Merging the whole store
    /// would write the stale snapshot over concurrent scenarios' promotions.
    promoted: std::collections::BTreeSet<String>,
}

impl World {
    /// A world over the given global store, with an empty scenario scope.
    pub fn new(global: GlobalStore) -> Self {
        Self {
            scenario: BTreeMap::new(),
            global,
            promoted: std::collections::BTreeSet::new(),
        }
    }

    /// Read a variable: the scenario scope shadows the global store.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.scenario.get(name).or_else(|| self.global.get(name))
    }

    /// Set a variable in the scenario scope.
    pub fn set(&mut self, name: impl Into<String>, value: Value) {
        self.scenario.insert(name.into(), value);
    }

    /// Promote a value into the persistent global store (`saveAs: global`).
    /// The key joins the scenario's write set (see [`World::promotions`]).
    pub fn set_global(&mut self, name: impl Into<String>, value: Value) {
        let name = name.into();
        self.promoted.insert(name.clone());
        self.global.insert(name, value);
    }

    /// The underlying global store.
    pub fn global(&self) -> &GlobalStore {
        &self.global
    }

    /// The scenario's promotions — only the keys actually written via
    /// [`World::set_global`], with their current values. This is what merges
    /// back into the shared store; the untouched snapshot remainder must not.
    pub fn promotions(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.promoted
            .iter()
            .filter_map(|key| self.global.get(key).map(|value| (key.as_str(), value)))
    }

    /// The merged view (globals overlaid by scenario values), in key order —
    /// the seed set for an engine's variable bridge.
    pub fn merged(&self) -> BTreeMap<&str, &Value> {
        let mut merged: BTreeMap<&str, &Value> = self.global.iter().collect();
        for (k, v) in &self.scenario {
            merged.insert(k.as_str(), v);
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn scenario_scope_shadows_global() {
        let mut store = GlobalStore::new();
        store.insert("token", Value::String("global".into()));
        let mut world = World::new(store);
        assert_eq!(world.get("token"), Some(&Value::String("global".into())));

        world.set("token", Value::String("scenario".into()));
        assert_eq!(world.get("token"), Some(&Value::String("scenario".into())));
    }

    #[test]
    fn merged_view_prefers_scenario_values() {
        let mut store = GlobalStore::new();
        store.insert("a", Value::Int(1));
        store.insert("b", Value::Int(2));
        let mut world = World::new(store);
        world.set("b", Value::Int(20));
        let merged = world.merged();
        assert_eq!(merged["a"], &Value::Int(1));
        assert_eq!(merged["b"], &Value::Int(20));
    }

    #[test]
    fn promotions_are_the_write_set_only() {
        let mut store = GlobalStore::new();
        store.insert("seed", Value::Int(1));
        let mut world = World::new(store);
        world.set("scenario-only", Value::Bool(true));
        world.set_global("promoted", Value::Int(2));

        let promotions: Vec<_> = world.promotions().collect();
        assert_eq!(promotions, vec![("promoted", &Value::Int(2))]);
    }

    #[test]
    fn value_json_forms_round_trip() {
        let cases = [
            ("null", Value::Null),
            ("true", Value::Bool(true)),
            ("3", Value::Int(3)),
            ("0.5", Value::Float(0.5)),
            (r#""c-42""#, Value::String("c-42".into())),
        ];
        for (json, expected) in cases {
            let parsed: Value = serde_json::from_str(json)
                .unwrap_or_else(|err| panic!("cannot parse {json}: {err}"));
            assert_eq!(parsed, expected, "for literal {json}");
        }
    }

    #[test]
    fn store_save_load_round_trips_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".proef-state.json");

        let mut store = GlobalStore::new();
        store.insert("clientId", Value::String("c-42".into()));
        store.insert("count", Value::Int(3));
        store.insert("ratio", Value::Float(0.5));
        store.save(&path).unwrap();

        // No temp residue after a successful save.
        assert!(!sibling_tmp_path(&path).exists());

        let loaded = GlobalStore::load(&path).unwrap();
        assert_eq!(loaded, store);
    }

    #[test]
    fn missing_state_file_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = GlobalStore::load(&dir.path().join("absent.json")).unwrap();
        assert_eq!(loaded, GlobalStore::new());
    }

    #[cfg(unix)]
    #[test]
    fn state_file_is_created_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".proef-state.json");
        GlobalStore::new().save(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn corrupt_state_file_is_a_system_fault() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".proef-state.json");
        fs::write(&path, "not json").unwrap();
        let err = GlobalStore::load(&path).unwrap_err();
        assert_eq!(err.exit_code().code(), 3);
    }
}
