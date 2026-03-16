use std::io::IsTerminal;

use serde_json::Value;

use crate::cli::OutputFormat;

pub struct RenderedOutput {
    pub format: OutputFormat,
    pub json: Value,
    pub table: String,
    pub exit_code: i32,
}

pub fn emit(rendered: RenderedOutput) {
    match effective_output_format(rendered.format, std::io::stdout().is_terminal()) {
        OutputFormat::Auto => unreachable!("auto format must resolve before emission"),
        OutputFormat::Json => println!("{}", rendered.json),
        OutputFormat::Table => println!("{}", rendered.table),
    }
}

fn effective_output_format(format: OutputFormat, stdout_is_terminal: bool) -> OutputFormat {
    match format {
        OutputFormat::Auto if stdout_is_terminal => OutputFormat::Table,
        OutputFormat::Auto => OutputFormat::Json,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_format_uses_table_for_terminals_and_json_for_pipes() {
        assert_eq!(
            effective_output_format(OutputFormat::Auto, true),
            OutputFormat::Table
        );
        assert_eq!(
            effective_output_format(OutputFormat::Auto, false),
            OutputFormat::Json
        );
    }
}
