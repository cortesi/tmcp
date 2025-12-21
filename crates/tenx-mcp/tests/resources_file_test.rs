//! Resource file loading tests.

#[cfg(test)]
mod tests {
    use std::{env::temp_dir, fs};

    use base64::{Engine, engine::general_purpose::STANDARD as Base64Standard};
    use tenx_mcp::schema::{ReadResourceResult, Resource, ResourceContents};

    #[test]
    fn test_read_resource_from_text_file() {
        // Create a temporary text file
        let temp_dir = temp_dir();
        let text_file = temp_dir.join("test_file.txt");
        fs::write(&text_file, "Hello, world!").unwrap();

        // Test with_file method
        let result = ReadResourceResult::new()
            .with_file(&text_file, "file://test.txt")
            .unwrap();

        assert_eq!(result.contents.len(), 1);
        match &result.contents[0] {
            ResourceContents::Text(text) => {
                assert_eq!(text.uri, "file://test.txt");
                assert_eq!(text.text, "Hello, world!");
                assert_eq!(text.mime_type, Some("text/plain".to_string()));
            }
            _ => panic!("Expected text content"),
        }

        // Clean up
        fs::remove_file(&text_file).unwrap();
    }

    #[test]
    fn test_read_resource_from_json_file() {
        // Create a temporary JSON file
        let temp_dir = temp_dir();
        let json_file = temp_dir.join("test_data.json");
        fs::write(&json_file, r#"{"name": "test", "value": 42}"#).unwrap();

        // Test with_file method
        let result = ReadResourceResult::new()
            .with_file(&json_file, "file://data.json")
            .unwrap();

        assert_eq!(result.contents.len(), 1);
        match &result.contents[0] {
            ResourceContents::Text(text) => {
                assert_eq!(text.uri, "file://data.json");
                assert_eq!(text.text, r#"{"name": "test", "value": 42}"#);
                assert_eq!(text.mime_type, Some("application/json".to_string()));
            }
            _ => panic!("Expected text content"),
        }

        // Clean up
        fs::remove_file(&json_file).unwrap();
    }

    #[test]
    fn test_read_resource_from_binary_file() {
        // Create a temporary binary file
        let temp_dir = temp_dir();
        let binary_file = temp_dir.join("test_image.png");
        let binary_data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]; // PNG header
        fs::write(&binary_file, &binary_data).unwrap();

        // Test with_file method
        let result = ReadResourceResult::new()
            .with_file(&binary_file, "file://image.png")
            .unwrap();

        assert_eq!(result.contents.len(), 1);
        match &result.contents[0] {
            ResourceContents::Blob(blob) => {
                assert_eq!(blob.uri, "file://image.png");
                assert_eq!(blob.mime_type, Some("image/png".to_string()));
                // Verify base64 encoding
                let decoded = Base64Standard.decode(&blob.blob).unwrap();
                assert_eq!(decoded, binary_data);
            }
            _ => panic!("Expected blob content"),
        }

        // Clean up
        fs::remove_file(&binary_file).unwrap();
    }

    #[test]
    fn test_read_multiple_files() {
        // Create temporary files
        let temp_dir = temp_dir();
        let file1 = temp_dir.join("file1.txt");
        let file2 = temp_dir.join("file2.md");

        fs::write(&file1, "Content 1").unwrap();
        fs::write(&file2, "# Markdown Content").unwrap();

        // Test with_files method
        let files = vec![
            (file1.as_path(), "file://file1.txt"),
            (file2.as_path(), "file://file2.md"),
        ];

        let result = ReadResourceResult::new().with_files(files).unwrap();

        assert_eq!(result.contents.len(), 2);

        // Verify first file
        match &result.contents[0] {
            ResourceContents::Text(text) => {
                assert_eq!(text.uri, "file://file1.txt");
                assert_eq!(text.text, "Content 1");
                assert_eq!(text.mime_type, Some("text/plain".to_string()));
            }
            _ => panic!("Expected text content for file1"),
        }

        // Verify second file
        match &result.contents[1] {
            ResourceContents::Text(text) => {
                assert_eq!(text.uri, "file://file2.md");
                assert_eq!(text.text, "# Markdown Content");
                assert_eq!(text.mime_type, Some("text/markdown".to_string()));
            }
            _ => panic!("Expected text content for file2"),
        }

        // Clean up
        fs::remove_file(&file1).unwrap();
        fs::remove_file(&file2).unwrap();
    }

    #[test]
    fn test_resource_from_file() {
        // Create a temporary file
        let temp_dir = temp_dir();
        let test_file = temp_dir.join("resource_test.rs");
        fs::write(&test_file, "fn main() {}").unwrap();

        // Test Resource::from_file method
        let resource = Resource::from_file(&test_file, "test-resource", "file://test.rs").unwrap();

        assert_eq!(resource.name, "test-resource");
        assert_eq!(resource.uri, "file://test.rs");
        assert_eq!(resource.mime_type, Some("text/x-rust".to_string()));
        assert_eq!(resource.size, Some(12)); // "fn main() {}" is 12 bytes

        // Clean up
        fs::remove_file(&test_file).unwrap();
    }

    #[test]
    fn test_resource_contents_from_file() {
        // Create a temporary XML file
        let temp_dir = temp_dir();
        let xml_file = temp_dir.join("test.xml");
        fs::write(&xml_file, "<?xml version=\"1.0\"?><root></root>").unwrap();

        // Test ResourceContents::from_file method
        let contents = ResourceContents::from_file(&xml_file, "file://test.xml").unwrap();

        match contents {
            ResourceContents::Text(text) => {
                assert_eq!(text.uri, "file://test.xml");
                assert_eq!(text.text, "<?xml version=\"1.0\"?><root></root>");
                assert_eq!(text.mime_type, Some("text/xml".to_string()));
            }
            _ => panic!("Expected text content for XML file"),
        }

        // Clean up
        fs::remove_file(&xml_file).unwrap();
    }
}
