use crate::converter::{ConversionOptions, ConversionResult, Converter};
use crate::error::ConvertError;

pub struct CodeConverter;

/// Map a file extension to its fenced code block language identifier.
fn language_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "c" | "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Some("cpp"),
        "py" | "pyw" => Some("python"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("jsx"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "rs" => Some("rust"),
        "go" => Some("go"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        "rb" => Some("ruby"),
        "swift" => Some("swift"),
        "cs" => Some("csharp"),
        "php" => Some("php"),
        "sh" | "bash" | "zsh" | "fish" => Some("bash"),
        "pl" | "pm" => Some("perl"),
        "lua" => Some("lua"),
        "r" => Some("r"),
        "scala" => Some("scala"),
        "dart" => Some("dart"),
        "ex" | "exs" => Some("elixir"),
        "erl" => Some("erlang"),
        "hs" => Some("haskell"),
        "ml" | "mli" => Some("ocaml"),
        "sql" => Some("sql"),
        "m" | "mm" => Some("objectivec"),
        "zig" => Some("zig"),
        "nim" => Some("nim"),
        "v" => Some("v"),
        "groovy" => Some("groovy"),
        "ps1" => Some("powershell"),
        "bat" | "cmd" => Some("batch"),
        _ => None,
    }
}

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "c", "h", "cpp", "cc", "cxx", "hpp", "hxx", "hh", "py", "pyw", "js", "mjs", "cjs", "jsx", "ts",
    "mts", "cts", "tsx", "rs", "go", "java", "kt", "kts", "rb", "swift", "cs", "php", "sh", "bash",
    "zsh", "fish", "pl", "pm", "lua", "r", "scala", "dart", "ex", "exs", "erl", "hs", "ml", "mli",
    "sql", "m", "mm", "zig", "nim", "v", "groovy", "ps1", "bat", "cmd",
];

impl Converter for CodeConverter {
    fn supported_extensions(&self) -> &[&str] {
        SUPPORTED_EXTENSIONS
    }

    fn convert(
        &self,
        data: &[u8],
        _options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        self.convert_with_extension(data, "code", _options)
    }
}

impl CodeConverter {
    /// Convert with a known file extension to produce the correct language tag.
    pub fn convert_with_extension(
        &self,
        data: &[u8],
        extension: &str,
        _options: &ConversionOptions,
    ) -> Result<ConversionResult, ConvertError> {
        let (text, warning) = super::decode_text(data);
        let mut warnings = Vec::new();
        if let Some(w) = warning {
            warnings.push(w);
        }

        let language = language_for_extension(extension).unwrap_or("code");
        let content = text.trim_end();
        let markdown = format!("```{language}\n{content}\n```\n");

        Ok(ConversionResult {
            markdown,
            warnings,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_python_fenced_block() {
        let converter = CodeConverter;
        let input = b"def hello():\n    print('Hello, world!')\n";
        let result = converter
            .convert_with_extension(input, "py", &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.starts_with("```python\n"));
        assert!(result.markdown.ends_with("\n```\n"));
        assert!(result.markdown.contains("def hello():"));
    }

    #[test]
    fn test_code_c_fenced_block() {
        let converter = CodeConverter;
        let input = b"#include <stdio.h>\nint main() { return 0; }\n";
        let result = converter
            .convert_with_extension(input, "c", &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.starts_with("```c\n"));
        assert!(result.markdown.contains("#include <stdio.h>"));
    }

    #[test]
    fn test_code_javascript_fenced_block() {
        let converter = CodeConverter;
        let input = b"console.log('hello');\n";
        let result = converter
            .convert_with_extension(input, "js", &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.starts_with("```javascript\n"));
        assert!(result.markdown.contains("console.log"));
    }

    #[test]
    fn test_code_empty_input() {
        let converter = CodeConverter;
        let result = converter
            .convert_with_extension(b"", "py", &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "```python\n\n```\n");
        // Whitespace-only input also produces empty code block
        let result = converter
            .convert_with_extension(b"  \n\n", "py", &ConversionOptions::default())
            .unwrap();
        assert_eq!(result.markdown, "```python\n\n```\n");
    }

    #[test]
    fn test_code_unicode_cjk() {
        let converter = CodeConverter;
        let input = "# 한국어 주석\nprint('中文')\n".as_bytes();
        let result = converter
            .convert_with_extension(input, "py", &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("한국어"));
        assert!(result.markdown.contains("中文"));
    }

    #[test]
    fn test_code_emoji() {
        let converter = CodeConverter;
        let input = "msg = '🚀✨🌍'\n".as_bytes();
        let result = converter
            .convert_with_extension(input, "py", &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("🚀✨🌍"));
    }

    #[test]
    fn test_code_non_utf8_decoded_with_warning() {
        let converter = CodeConverter;
        // Windows-1252 encoded: "café" with é = 0xE9
        let input = b"caf\xe9";
        let result = converter
            .convert_with_extension(input, "py", &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.contains("café"));
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(
            result.warnings[0].code,
            crate::converter::WarningCode::UnsupportedFeature
        );
    }

    #[test]
    fn test_code_supported_extensions() {
        let converter = CodeConverter;
        let exts = converter.supported_extensions();
        assert!(exts.contains(&"py"));
        assert!(exts.contains(&"rs"));
        assert!(exts.contains(&"js"));
        assert!(exts.contains(&"c"));
        assert!(exts.contains(&"go"));
        assert!(exts.contains(&"java"));
        assert!(exts.contains(&"sh"));
        assert!(exts.contains(&"sql"));
        assert!(exts.contains(&"zig"));
        assert!(exts.contains(&"bat"));
        assert!(!exts.contains(&"txt"));
        assert!(!exts.contains(&"docx"));
    }

    #[test]
    fn test_code_can_convert() {
        let converter = CodeConverter;
        assert!(converter.can_convert("py", &[]));
        assert!(converter.can_convert("rs", &[]));
        assert!(converter.can_convert("js", &[]));
        assert!(!converter.can_convert("txt", &[]));
        assert!(!converter.can_convert("docx", &[]));
        assert!(!converter.can_convert("json", &[]));
    }

    #[test]
    fn test_code_no_title_or_images() {
        let converter = CodeConverter;
        let result = converter
            .convert_with_extension(b"x = 1", "py", &ConversionOptions::default())
            .unwrap();
        assert!(result.title.is_none());
        assert!(result.images.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_code_header_extension_mapping() {
        let converter = CodeConverter;

        let result = converter
            .convert_with_extension(b"int x;", "h", &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.starts_with("```c\n"));

        let result = converter
            .convert_with_extension(b"int x;", "hpp", &ConversionOptions::default())
            .unwrap();
        assert!(result.markdown.starts_with("```cpp\n"));
    }

    #[test]
    fn test_code_backtick_content_not_broken() {
        let converter = CodeConverter;
        let input = b"code = '''```triple backticks```'''\n";
        let result = converter
            .convert_with_extension(input, "py", &ConversionOptions::default())
            .unwrap();
        // The fenced block structure should remain valid —
        // content with backticks is preserved as-is
        assert!(result.markdown.starts_with("```python\n"));
        assert!(result.markdown.contains("```triple backticks```"));
        assert!(result.markdown.ends_with("\n```\n"));
    }
}
