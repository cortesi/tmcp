use std::collections::HashMap;

use serde::{
    Deserialize, Serialize,
    de::{DeserializeOwned, Error as DeError},
};
use serde_json::{Map, Value};

/// Generic argument map used for passing parameters to tools and prompts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Arguments(pub(crate) Map<String, Value>);

impl Arguments {
    /// Create an empty argument set.
    pub fn new() -> Self {
        Self(Map::new())
    }

    /// Build arguments from any serializable struct.
    pub fn from_struct<T: Serialize>(value: T) -> Result<Self, serde_json::Error> {
        match serde_json::to_value(value)? {
            Value::Object(map) => Ok(Self(map)),
            Value::Null => Ok(Self::new()),
            _ => Err(DeError::custom("arguments must be a struct")),
        }
    }

    /// Insert a single key/value pair, returning the updated `Arguments`.
    ///
    /// This enables fluent construction without an intermediate `HashMap`.
    pub fn set(
        mut self,
        key: impl Into<String>,
        value: impl Serialize,
    ) -> Result<Self, serde_json::Error> {
        let v = serde_json::to_value(value)?;
        self.0.insert(key.into(), v);
        Ok(self)
    }

    /// Deserialize the arguments into the desired type.
    pub fn deserialize<T: DeserializeOwned>(self) -> Result<T, serde_json::Error> {
        serde_json::from_value(Value::Object(self.0))
    }

    /// Get a typed value by key.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.0
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Get the raw JSON value for a key.
    pub fn get_value(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    /// Get a string value by key.
    pub fn get_string(&self, key: &str) -> Option<String> {
        self.get::<String>(key)
    }

    /// Get an i64 value by key.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get::<i64>(key)
    }

    /// Get a bool value by key.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get::<bool>(key)
    }
}

impl From<HashMap<String, Value>> for Arguments {
    fn from(map: HashMap<String, Value>) -> Self {
        Self(map.into_iter().collect())
    }
}

impl From<HashMap<String, String>> for Arguments {
    fn from(map: HashMap<String, String>) -> Self {
        Self(
            map.into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect(),
        )
    }
}
