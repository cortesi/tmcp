use std::{slice, vec};

use base64::{Engine, engine::general_purpose::STANDARD as Base64Standard};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, de::DeserializeOwned, ser};
use serde_json::{Map, Value};

use super::{Resource, ResourceContents};
use crate::{Arguments, macros::with_meta};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A content block for prompts, tool results, and resources.
#[derive(Debug, Clone)]
pub enum ContentBlock {
    /// Text content.
    Text(TextContent),
    /// Base64-encoded image content.
    Image(ImageContent),
    /// Base64-encoded audio content.
    Audio(AudioContent),
    /// Link to an MCP resource.
    ResourceLink(ResourceLink),
    /// Embedded resource contents.
    EmbeddedResource(EmbeddedResource),
    /// Unknown future content block variant.
    Unknown(UnknownContentBlock),
}

impl ContentBlock {
    /// Create a new text content block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextContent::new(text))
    }

    /// Create a new image content block.
    pub fn image(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self::Image(ImageContent::new(data, mime_type))
    }

    /// Create a new audio content block.
    pub fn audio(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self::Audio(AudioContent::new(data, mime_type))
    }

    /// Create a new embedded resource content block.
    pub fn resource(resource: ResourceContents) -> Self {
        Self::EmbeddedResource(EmbeddedResource {
            resource,
            annotations: None,
            _meta: None,
        })
    }
}

impl Serialize for ContentBlock {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_content_block(self, serializer)
    }
}

impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_content_block(deserializer, known_content_block)
    }
}

/// A content block for sampling messages.
#[derive(Debug, Clone)]
pub enum SamplingMessageContentBlock {
    /// Text content.
    Text(TextContent),
    /// Base64-encoded image content.
    Image(ImageContent),
    /// Base64-encoded audio content.
    Audio(AudioContent),
    /// A tool-use request.
    ToolUse(ToolUseContent),
    /// A tool-use result.
    ToolResult(ToolResultContent),
    /// Unknown future content block variant.
    Unknown(UnknownContentBlock),
}

impl Serialize for SamplingMessageContentBlock {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_sampling_content_block(self, serializer)
    }
}

impl<'de> Deserialize<'de> for SamplingMessageContentBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_content_block(deserializer, known_sampling_content_block)
    }
}

/// Unknown future content block variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnknownContentBlock {
    /// Literal content type string from the wire payload.
    #[serde(rename = "type")]
    pub content_type: String,
    /// Unknown payload fields other than `type`.
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

impl UnknownContentBlock {
    /// Create an unknown content block.
    pub fn new(content_type: impl Into<String>, fields: Map<String, Value>) -> Self {
        Self {
            content_type: content_type.into(),
            fields,
        }
    }
}

/// Container that can represent either a single value or a list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    /// Returns an iterator over the contained values.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        let s: &[T] = match self {
            Self::One(value) => slice::from_ref(value),
            Self::Many(values) => values.as_slice(),
        };
        s.iter()
    }

    /// Returns a mutable iterator over the contained values.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        let s: &mut [T] = match self {
            Self::One(value) => slice::from_mut(value),
            Self::Many(values) => values.as_mut_slice(),
        };
        s.iter_mut()
    }

    /// Returns a reference to the first element.
    pub fn first(&self) -> Option<&T> {
        match self {
            Self::One(value) => Some(value),
            Self::Many(values) => values.first(),
        }
    }

    /// Returns the number of elements.
    pub fn len(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Many(values) => values.len(),
        }
    }

    /// Returns true if there are no elements.
    pub fn is_empty(&self) -> bool {
        match self {
            Self::One(_) => false,
            Self::Many(values) => values.is_empty(),
        }
    }

    /// Converts into a Vec, consuming self.
    pub fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

impl<T> IntoIterator for OneOrMany<T> {
    type Item = T;
    type IntoIter = vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}

impl<'a, T> IntoIterator for &'a OneOrMany<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            OneOrMany::One(value) => slice::from_ref(value).iter(),
            OneOrMany::Many(values) => values.iter(),
        }
    }
}

impl<T> From<T> for OneOrMany<T> {
    fn from(value: T) -> Self {
        Self::One(value)
    }
}

impl<T> From<Vec<T>> for OneOrMany<T> {
    fn from(values: Vec<T>) -> Self {
        Self::Many(values)
    }
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedResource {
    pub resource: ResourceContents,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

/// A resource that the server is capable of reading, included in a prompt or tool call result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLink {
    #[serde(flatten)]
    pub resource: Resource,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Annotations {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<Role>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    #[serde(rename = "lastModified", skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}

impl Annotations {
    pub fn new() -> Self {
        Self {
            audience: None,
            priority: None,
            last_modified: None,
        }
    }

    pub fn with_audience(mut self, audience: Vec<Role>) -> Self {
        self.audience = Some(audience);
        self
    }

    pub fn with_priority(mut self, priority: f64) -> Self {
        self.priority = Some(priority);
        self
    }

    pub fn with_last_modified(mut self, last_modified: impl Into<String>) -> Self {
        self.last_modified = Some(last_modified.into());
        self
    }
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextContent {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

impl TextContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            annotations: None,
            _meta: None,
        }
    }

    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContent {
    pub data: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

impl ImageContent {
    pub fn new(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            mime_type: mime_type.into(),
            annotations: None,
            _meta: None,
        }
    }

    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }

    /// Decode the base64 image data.
    pub fn data_bytes(&self) -> Result<Vec<u8>, base64::DecodeError> {
        Base64Standard.decode(&self.data)
    }

    /// Replace image data with base64-encoded bytes.
    pub fn with_data_bytes(mut self, data: &[u8]) -> Self {
        self.data = Base64Standard.encode(data);
        self
    }
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioContent {
    pub data: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

impl AudioContent {
    pub fn new(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            mime_type: mime_type.into(),
            annotations: None,
            _meta: None,
        }
    }

    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }

    /// Decode the base64 audio data.
    pub fn data_bytes(&self) -> Result<Vec<u8>, base64::DecodeError> {
        Base64Standard.decode(&self.data)
    }

    /// Replace audio data with base64-encoded bytes.
    pub fn with_data_bytes(mut self, data: &[u8]) -> Self {
        self.data = Base64Standard.encode(data);
        self
    }
}

/// Serialize a prompt/tool/resource content block with its `type` tag.
fn serialize_content_block<S>(block: &ContentBlock, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match block {
        ContentBlock::Text(content) => serialize_tagged_content("text", content, serializer),
        ContentBlock::Image(content) => serialize_tagged_content("image", content, serializer),
        ContentBlock::Audio(content) => serialize_tagged_content("audio", content, serializer),
        ContentBlock::ResourceLink(content) => {
            serialize_tagged_content("resource_link", content, serializer)
        }
        ContentBlock::EmbeddedResource(content) => {
            serialize_tagged_content("resource", content, serializer)
        }
        ContentBlock::Unknown(content) => content.serialize(serializer),
    }
}

/// Serialize a sampling content block with its `type` tag.
fn serialize_sampling_content_block<S>(
    block: &SamplingMessageContentBlock,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match block {
        SamplingMessageContentBlock::Text(content) => {
            serialize_tagged_content("text", content, serializer)
        }
        SamplingMessageContentBlock::Image(content) => {
            serialize_tagged_content("image", content, serializer)
        }
        SamplingMessageContentBlock::Audio(content) => {
            serialize_tagged_content("audio", content, serializer)
        }
        SamplingMessageContentBlock::ToolUse(content) => {
            serialize_tagged_content("tool_use", content, serializer)
        }
        SamplingMessageContentBlock::ToolResult(content) => {
            serialize_tagged_content("tool_result", content, serializer)
        }
        SamplingMessageContentBlock::Unknown(content) => content.serialize(serializer),
    }
}

/// Serialize one tagged content payload.
fn serialize_tagged_content<S>(
    content_type: &str,
    content: &impl Serialize,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut value = serde_json::to_value(content).map_err(ser::Error::custom)?;
    let Value::Object(object) = &mut value else {
        return Err(ser::Error::custom("content block must serialize as object"));
    };
    object.insert("type".to_owned(), Value::String(content_type.to_owned()));
    value.serialize(serializer)
}

/// Deserialize one tagged content block using the provided known-variant mapping.
fn deserialize_content_block<'de, D, T, F>(deserializer: D, known: F) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    F: FnOnce(&str, Map<String, Value>) -> Result<T, serde_json::Error>,
{
    let mut object = Map::<String, Value>::deserialize(deserializer)?;
    let content_type = object
        .remove("type")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| de::Error::custom("content block missing string `type`"))?;
    known(&content_type, object).map_err(de::Error::custom)
}

/// Convert one content-block object to a known content block or unknown fallback.
fn known_content_block(
    content_type: &str,
    object: Map<String, Value>,
) -> Result<ContentBlock, serde_json::Error> {
    match content_type {
        "text" => typed_content(object).map(ContentBlock::Text),
        "image" => typed_content(object).map(ContentBlock::Image),
        "audio" => typed_content(object).map(ContentBlock::Audio),
        "resource_link" => typed_content(object).map(ContentBlock::ResourceLink),
        "resource" => typed_content(object).map(ContentBlock::EmbeddedResource),
        _ => Ok(ContentBlock::Unknown(UnknownContentBlock::new(
            content_type,
            object,
        ))),
    }
}

/// Convert one content-block object to a known sampling content block or unknown fallback.
fn known_sampling_content_block(
    content_type: &str,
    object: Map<String, Value>,
) -> Result<SamplingMessageContentBlock, serde_json::Error> {
    match content_type {
        "text" => typed_content(object).map(SamplingMessageContentBlock::Text),
        "image" => typed_content(object).map(SamplingMessageContentBlock::Image),
        "audio" => typed_content(object).map(SamplingMessageContentBlock::Audio),
        "tool_use" => typed_content(object).map(SamplingMessageContentBlock::ToolUse),
        "tool_result" => typed_content(object).map(SamplingMessageContentBlock::ToolResult),
        _ => Ok(SamplingMessageContentBlock::Unknown(
            UnknownContentBlock::new(content_type, object),
        )),
    }
}

/// Deserialize a known content object after removing its tag.
fn typed_content<T: DeserializeOwned>(object: Map<String, Value>) -> Result<T, serde_json::Error> {
    serde_json::from_value(Value::Object(object))
}

/// A request from the assistant to call a tool.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseContent {
    /// A unique identifier for this tool use.
    pub id: String,
    /// The name of the tool to call.
    pub name: String,
    /// The arguments to pass to the tool.
    pub input: Arguments,
}

/// The result of a tool use, provided back to the assistant.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultContent {
    /// The ID of the tool use this result corresponds to.
    #[serde(rename = "toolUseId")]
    pub tool_use_id: String,
    /// The unstructured result content of the tool use.
    pub content: Vec<ContentBlock>,
    /// An optional structured result object.
    #[serde(rename = "structuredContent", skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    /// Whether the tool use resulted in an error.
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_block_helpers() {
        let text = ContentBlock::text("hello");
        assert!(matches!(text, ContentBlock::Text(_)));

        let image = ContentBlock::image("data", "image/png");
        assert!(matches!(image, ContentBlock::Image(_)));

        let audio = ContentBlock::audio("data", "audio/mpeg");
        assert!(matches!(audio, ContentBlock::Audio(_)));
    }

    #[test]
    fn content_block_preserves_unknown_variants() {
        let value = serde_json::json!({
            "type": "video",
            "data": "AAAA",
            "mimeType": "video/mp4",
            "codec": "h264"
        });

        let block: ContentBlock = serde_json::from_value(value.clone()).expect("unknown block");
        let ContentBlock::Unknown(unknown) = block else {
            panic!("expected unknown content block");
        };
        assert_eq!(unknown.content_type, "video");
        assert_eq!(
            unknown.fields.get("mimeType"),
            Some(&Value::String("video/mp4".to_owned()))
        );

        let encoded = serde_json::to_value(ContentBlock::Unknown(unknown)).expect("serialize");
        assert_eq!(encoded, value);
    }

    #[test]
    fn sampling_content_block_preserves_unknown_variants() {
        let value = serde_json::json!({
            "type": "transcript",
            "language": "en",
            "segments": [{ "start": 0, "text": "hello" }]
        });

        let block: SamplingMessageContentBlock =
            serde_json::from_value(value.clone()).expect("unknown sampling block");
        let SamplingMessageContentBlock::Unknown(unknown) = block else {
            panic!("expected unknown sampling content block");
        };
        assert_eq!(unknown.content_type, "transcript");

        let encoded =
            serde_json::to_value(SamplingMessageContentBlock::Unknown(unknown)).expect("serialize");
        assert_eq!(encoded, value);
    }

    #[test]
    fn binary_content_helpers_encode_and_decode_bytes() {
        let bytes = b"\x00tmcp image bytes";

        let image = ImageContent::new("", "image/png").with_data_bytes(bytes);
        assert_eq!(image.data_bytes().expect("decode image"), bytes);

        let audio = AudioContent::new("", "audio/mpeg").with_data_bytes(bytes);
        assert_eq!(audio.data_bytes().expect("decode audio"), bytes);
    }
}
