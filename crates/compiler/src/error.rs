use std::fmt;

use proc_macro2::Span;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    pub line: usize,
    pub column: usize,
    /// Module path the position belongs to, when it is not the compiled file.
    pub file: Option<String>,
    /// Set when the error must stop the build even in `auto` mode.
    pub fatal: bool,
}

impl Error {
    pub(crate) fn at(span: Span, message: impl Into<String>) -> Self {
        let start = span.start();
        Self {
            message: message.into(),
            line: start.line,
            column: start.column.saturating_add(1),
            file: None,
            fatal: false,
        }
    }

    pub(crate) fn fatal(span: Span, message: impl Into<String>) -> Self {
        let mut error = Self::at(span, message);
        error.fatal = true;
        error
    }

    pub(crate) fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            line: 0,
            column: 0,
            file: None,
            fatal: false,
        }
    }

    pub(crate) fn with_file(mut self, module: &str) -> Self {
        self.file = Some(module.to_owned());
        self
    }

    #[must_use]
    pub fn render(&self, file: &str) -> String {
        let file = self.file.as_deref().unwrap_or(file);
        if self.line == 0 {
            format!("{file}: {}", self.message)
        } else {
            format!("{file}:{}:{}: {}", self.line, self.column, self.message)
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(formatter, "{}", self.message)
        } else {
            write!(
                formatter,
                "line {}, column {}: {}",
                self.line, self.column, self.message
            )
        }
    }
}

impl std::error::Error for Error {}

impl From<syn::Error> for Error {
    fn from(error: syn::Error) -> Self {
        Self::at(error.span(), error.to_string())
    }
}
