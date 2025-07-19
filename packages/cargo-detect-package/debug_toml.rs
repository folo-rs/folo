fn main() {
    let test_content = r#"
[package]
name = "test-package"
version = "0.1.0"
"#;
    
    println!("Original content: {:?}", test_content);
    println!("Trimmed content: {:?}", test_content.trim());
    
    match toml::from_str::<toml::Value>(test_content) {
        Ok(value) => println!("Original parse successful: {:#?}", value),
        Err(e) => println!("Original parse error: {}", e),
    }
    
    match toml::from_str::<toml::Value>(test_content.trim()) {
        Ok(value) => println!("Trimmed parse successful: {:#?}", value),
        Err(e) => println!("Trimmed parse error: {}", e),
    }
}
