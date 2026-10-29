//! What the operator declares in Brama's environment, read one way: an unset
//! variable declares nothing, and a variable set to text that is not Unicode
//! is refused by name, never read as unset. Every caller says what an
//! undeclared value means for it.

/// The text `variable` declares, as set (callers decide what blank means);
/// `None` when it is unset, refused by name when it is not Unicode.
pub fn declared_text(variable: &str) -> Result<Option<String>, String> {
    match std::env::var(variable) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(value)) => Err(format!(
            "{variable} is set to text that is not Unicode: {value:?}"
        )),
    }
}

/// The text `variable` declares, trimmed, when it names something: `None`
/// when it is unset or blank, refused by name when it is not Unicode.
pub fn declared_setting(variable: &str) -> Result<Option<String>, String> {
    Ok(declared_text(variable)?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

/// The Stado binary Brama asks: `BRAMA_STADO_BIN`, else `.stado/bin/stado`
/// under the user's home, never under the working directory. A declaration
/// that cannot be read is refused by name, not passed over for the home copy.
pub fn stado_binary() -> Result<std::path::PathBuf, String> {
    Ok(match declared_setting("BRAMA_STADO_BIN")? {
        Some(value) => std::path::PathBuf::from(value),
        None => crate::journal::home()?
            .join(".stado")
            .join("bin")
            .join("stado"),
    })
}
