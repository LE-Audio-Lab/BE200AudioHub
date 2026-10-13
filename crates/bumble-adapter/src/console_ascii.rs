/// Normalize adapter diagnostics to ASCII before they enter Windows
/// PowerShell 5.1 native-process pipes.
pub(crate) fn sanitize_console_text(input: &str) -> String {
    if input.is_ascii() {
        return input.to_owned();
    }

    input
        .replace('—', "-")
        .replace('–', "-")
        .replace('→', "->")
        .replace('…', "...")
}

macro_rules! println {
    () => {
        std::println!()
    };
    ($($arg:tt)*) => {{
        let rendered = format!($($arg)*);
        std::println!("{}", crate::console_ascii::sanitize_console_text(&rendered));
    }};
}

macro_rules! eprintln {
    () => {
        std::eprintln!()
    };
    ($($arg:tt)*) => {{
        let rendered = format!($($arg)*);
        std::eprintln!("{}", crate::console_ascii::sanitize_console_text(&rendered));
    }};
}

#[cfg(test)]
mod tests {
    use super::sanitize_console_text;

    #[test]
    fn normalizes_cli_punctuation() {
        assert_eq!(sanitize_console_text("A — B → C … D – E"), "A - B -> C ... D - E");
    }
}
