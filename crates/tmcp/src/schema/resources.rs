#![allow(missing_docs)]

use std::{fs, path::Path};

use base64::{Engine, engine::general_purpose::STANDARD as Base64Standard};
use mime_guess::mime::{APPLICATION, TEXT};
use serde::{Deserialize, Serialize};

use super::*;
use crate::{
    Result,
    macros::{with_basename, with_meta},
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListResourcesResult {
    pub resources: Vec<Resource>,
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

impl ListResourcesResult {
    pub fn new() -> Self {
        Self {
            resources: Vec::new(),
            next_cursor: None,
        }
    }

    pub fn with_resource(mut self, resource: Resource) -> Self {
        self.resources.push(resource);
        self
    }

    pub fn with_resources(mut self, resources: impl IntoIterator<Item = Resource>) -> Self {
        self.resources.extend(resources);
        self
    }

    pub fn with_cursor(mut self, cursor: impl Into<Cursor>) -> Self {
        self.next_cursor = Some(cursor.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListResourceTemplatesResult {
    #[serde(rename = "resourceTemplates")]
    pub resource_templates: Vec<ResourceTemplate>,
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

impl ListResourceTemplatesResult {
    pub fn new() -> Self {
        Self {
            resource_templates: Vec::new(),
            next_cursor: None,
        }
    }

    pub fn with_resource_template(mut self, template: ResourceTemplate) -> Self {
        self.resource_templates.push(template);
        self
    }

    pub fn with_resource_templates(
        mut self,
        templates: impl IntoIterator<Item = ResourceTemplate>,
    ) -> Self {
        self.resource_templates.extend(templates);
        self
    }

    pub fn with_cursor(mut self, cursor: impl Into<Cursor>) -> Self {
        self.next_cursor = Some(cursor.into());
        self
    }
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReadResourceResult {
    pub contents: Vec<ResourceContents>,
}

impl ReadResourceResult {
    pub fn new() -> Self {
        Self {
            contents: Vec::new(),
            _meta: None,
        }
    }

    pub fn with_content(mut self, content: ResourceContents) -> Self {
        self.contents.push(content);
        self
    }

    pub fn with_contents(mut self, contents: impl IntoIterator<Item = ResourceContents>) -> Self {
        self.contents.extend(contents);
        self
    }

    pub fn with_text(mut self, uri: impl Into<String>, text: impl Into<String>) -> Self {
        self.contents.push(ResourceContents::text(uri, text));
        self
    }

    pub fn with_json<T: Serialize>(mut self, uri: impl Into<String>, value: &T) -> Result<Self> {
        let content = ResourceContents::json(uri, value)?;
        self.contents.push(content);
        Ok(self)
    }

    pub fn with_file<P: AsRef<Path>>(mut self, path: P, uri: impl Into<String>) -> Result<Self> {
        let content = ResourceContents::from_file(path, uri)?;
        self.contents.push(content);
        Ok(self)
    }

    pub fn with_files<P, U>(mut self, paths: impl IntoIterator<Item = (P, U)>) -> Result<Self>
    where
        P: AsRef<Path>,
        U: Into<String>,
    {
        for (path, uri) in paths {
            let content = ResourceContents::from_file(path, uri)?;
            self.contents.push(content);
        }
        Ok(self)
    }
}

/// A known resource that the server is capable of reading.
#[with_meta]
#[with_basename]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
}

impl Resource {
    pub fn new(name: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            description: None,
            mime_type: None,
            annotations: None,
            size: None,
            icons: None,
            name: name.into(),
            title: None,
            _meta: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }

    pub fn with_size(mut self, size: i64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_icons(mut self, icons: impl IntoIterator<Item = Icon>) -> Self {
        self.icons = Some(icons.into_iter().collect());
        self
    }

    pub fn from_file<P: AsRef<Path>>(
        path: P,
        name: impl Into<String>,
        uri: impl Into<String>,
    ) -> Result<Self> {
        let path = path.as_ref();
        let metadata = fs::metadata(path)?;
        let mime_type = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();

        Ok(Self::new(name, uri)
            .with_mime_type(mime_type)
            .with_size(metadata.len() as i64))
    }
}

/// A template description for resources available on the server.
#[with_meta]
#[with_basename]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceTemplate {
    #[serde(rename = "uriTemplate")]
    pub uri_template: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
}

impl ResourceTemplate {
    pub fn new(name: impl Into<String>, uri_template: impl Into<String>) -> Self {
        Self {
            uri_template: uri_template.into(),
            description: None,
            mime_type: None,
            annotations: None,
            icons: None,
            name: name.into(),
            title: None,
            _meta: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    pub fn with_annotations(mut self, annotations: Annotations) -> Self {
        self.annotations = Some(annotations);
        self
    }

    pub fn with_icons(mut self, icons: impl IntoIterator<Item = Icon>) -> Self {
        self.icons = Some(icons.into_iter().collect());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceContents {
    Text(TextResourceContents),
    Blob(BlobResourceContents),
}

impl ResourceContents {
    pub fn text(uri: impl Into<String>, text: impl Into<String>) -> Self {
        Self::Text(TextResourceContents {
            uri: uri.into(),
            mime_type: None,
            text: text.into(),
            _meta: None,
        })
    }

    pub fn blob(uri: impl Into<String>, blob: impl Into<String>) -> Self {
        Self::Blob(BlobResourceContents {
            uri: uri.into(),
            mime_type: None,
            blob: blob.into(),
            _meta: None,
        })
    }

    pub fn json<T: Serialize>(uri: impl Into<String>, value: &T) -> Result<Self> {
        let json_text = serde_json::to_string_pretty(value)?;
        Ok(Self::Text(TextResourceContents {
            uri: uri.into(),
            mime_type: Some("application/json".to_string()),
            text: json_text,
            _meta: None,
        }))
    }

    pub fn from_file<P: AsRef<Path>>(path: P, uri: impl Into<String>) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read(path)?;
        let mime_type = mime_guess::from_path(path).first_or_octet_stream();
        let uri_string = uri.into();

        // Determine if the content should be treated as text or binary
        // Based on the MIME type's primary type
        match mime_type.type_() {
            TEXT | APPLICATION => {
                // Try to read as UTF-8 text
                match String::from_utf8(contents.clone()) {
                    Ok(text) => {
                        // Special handling for common text-based application types
                        let is_text_app = mime_type.subtype() == "json"
                            || mime_type.subtype() == "xml"
                            || mime_type.subtype() == "javascript"
                            || mime_type.subtype() == "x-yaml"
                            || mime_type.subtype() == "yaml";

                        if mime_type.type_() == TEXT || is_text_app {
                            Ok(Self::Text(TextResourceContents {
                                uri: uri_string,
                                mime_type: Some(mime_type.to_string()),
                                text,
                                _meta: None,
                            }))
                        } else {
                            // Binary application type
                            Ok(Self::Blob(BlobResourceContents {
                                uri: uri_string,
                                mime_type: Some(mime_type.to_string()),
                                blob: Base64Standard.encode(text.as_bytes()),
                                _meta: None,
                            }))
                        }
                    }
                    Err(_) => {
                        // Not valid UTF-8, treat as binary
                        Ok(Self::Blob(BlobResourceContents {
                            uri: uri_string,
                            mime_type: Some(mime_type.to_string()),
                            blob: Base64Standard.encode(&contents),
                            _meta: None,
                        }))
                    }
                }
            }
            _ => {
                // All other types (IMAGE, AUDIO, VIDEO, etc.) are binary
                Ok(Self::Blob(BlobResourceContents {
                    uri: uri_string,
                    mime_type: Some(mime_type.to_string()),
                    blob: Base64Standard.encode(&contents),
                    _meta: None,
                }))
            }
        }
    }
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextResourceContents {
    pub uri: String,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub text: String,
}

impl TextResourceContents {
    pub fn new(uri: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            mime_type: None,
            text: text.into(),
            _meta: None,
        }
    }

    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobResourceContents {
    pub uri: String,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub blob: String,
}

impl BlobResourceContents {
    pub fn new(uri: impl Into<String>, blob: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            mime_type: None,
            blob: blob.into(),
            _meta: None,
        }
    }

    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }
}
