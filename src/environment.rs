use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

const FILE_LIMIT: usize = 64 * 1024;

pub(super) enum ConfigurationLayout {
    Unix,
    Windows,
}

pub(super) struct Assignments {
    pub(super) endpoint: Option<String>,
    pub(super) token: Option<String>,
    pub(super) token_origin: Option<String>,
}

pub(super) enum FileError {
    Unreadable,
    NotRegular,
    Permissive,
    TooLarge,
    NotUtf8,
    Malformed,
    UnknownKey,
    DuplicateKey,
}

pub(super) fn default_path(
    layout: ConfigurationLayout,
    xdg_config_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
) -> Option<PathBuf> {
    if let Some(directory) = present(xdg_config_home) {
        return Some(PathBuf::from(directory).join("bass").join("environment"));
    }
    match layout {
        ConfigurationLayout::Windows => present(appdata)
            .map(|directory| PathBuf::from(directory).join("bass").join("environment")),
        ConfigurationLayout::Unix => present(home).map(|directory| {
            PathBuf::from(directory)
                .join(".config")
                .join("bass")
                .join("environment")
        }),
    }
}

pub(super) fn load(path: &Path, required: bool) -> Result<Option<Assignments>, FileError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound && !required => return Ok(None),
        Err(_) => return Err(FileError::Unreadable),
    };
    permit_only_the_owner(&file)?;
    let text = read_text(file)?;
    parse(&text).map(Some)
}

pub(super) fn parse(text: &str) -> Result<Assignments, FileError> {
    let mut endpoint = None;
    let mut token = None;
    let mut token_origin = None;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = line.trim_start_matches([' ', '\t']);
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(FileError::Malformed);
        };
        if !key_shape(key) {
            return Err(FileError::Malformed);
        }
        let slot = match key {
            "BASS_ENDPOINT" => &mut endpoint,
            "BASS_TOKEN" => &mut token,
            "BASS_TOKEN_ORIGIN" => &mut token_origin,
            _ => return Err(FileError::UnknownKey),
        };
        if slot.is_some() {
            return Err(FileError::DuplicateKey);
        }
        *slot = Some(value.to_string());
    }
    Ok(Assignments {
        endpoint,
        token,
        token_origin,
    })
}

fn present(value: Option<&str>) -> Option<&str> {
    match value {
        Some(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

fn key_shape(key: &str) -> bool {
    let mut characters = key.chars();
    match characters.next() {
        Some(character) if character.is_ascii_alphabetic() || character == '_' => {}
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn permit_only_the_owner(file: &File) -> Result<(), FileError> {
    let metadata = file.metadata().map_err(|_| FileError::Unreadable)?;
    if !metadata.is_file() {
        return Err(FileError::NotRegular);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(FileError::Permissive);
        }
    }
    Ok(())
}

fn read_text(mut file: File) -> Result<String, FileError> {
    let mut buffer = Vec::new();
    file.by_ref()
        .take((FILE_LIMIT as u64) + 1)
        .read_to_end(&mut buffer)
        .map_err(|_| FileError::Unreadable)?;
    if buffer.len() > FILE_LIMIT {
        return Err(FileError::TooLarge);
    }
    String::from_utf8(buffer).map_err(|_| FileError::NotUtf8)
}

impl std::fmt::Display for FileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            FileError::Unreadable => "The environment file could not be read.",
            FileError::NotRegular => "The environment file is not a regular file.",
            FileError::Permissive => "The environment file permits group or other access.",
            FileError::TooLarge => "The environment file is too large.",
            FileError::NotUtf8 => "The environment file is not valid text.",
            FileError::Malformed => "The environment file has a malformed line.",
            FileError::UnknownKey => "The environment file has an unknown key.",
            FileError::DuplicateKey => "The environment file repeats a key.",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> String {
        match parse(text) {
            Ok(_) => panic!("expected an environment file error"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn parses_literal_values_comments_and_blank_lines() {
        let assignments = match parse(
            "\n# comment\n  # indented comment\r\nBASS_ENDPOINT=https://gateway.example/v1\r\nBASS_TOKEN=$(printf secret) # stays\n",
        ) {
            Ok(assignments) => assignments,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(
            assignments.endpoint.as_deref(),
            Some("https://gateway.example/v1")
        );
        assert_eq!(
            assignments.token.as_deref(),
            Some("$(printf secret) # stays")
        );
        assert_eq!(assignments.token_origin, None);
        let with_origin = match parse("BASS_TOKEN_ORIGIN=https://gateway.example\n") {
            Ok(assignments) => assignments,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(
            with_origin.token_origin.as_deref(),
            Some("https://gateway.example")
        );
    }

    #[test]
    fn rejects_unknown_duplicate_and_malformed_lines() {
        assert_eq!(
            message("BASS_MODEL=demo\n"),
            "The environment file has an unknown key."
        );
        assert_eq!(
            message("BASS_TOKEN=one\nBASS_TOKEN=two\n"),
            "The environment file repeats a key."
        );
        assert_eq!(
            message("BASS_TOKEN\n"),
            "The environment file has a malformed line."
        );
        assert_eq!(
            message("  BASS_TOKEN=secret\n"),
            "The environment file has a malformed line."
        );
    }

    #[test]
    fn resolves_the_default_path() {
        let unix = default_path(
            ConfigurationLayout::Unix,
            None,
            Some("/home/operator"),
            Some("C:\\Users\\operator\\AppData\\Roaming"),
        );
        assert_eq!(
            unix,
            Some(PathBuf::from("/home/operator/.config/bass/environment"))
        );
        let xdg = default_path(
            ConfigurationLayout::Windows,
            Some("/custom/config"),
            Some("/home/operator"),
            Some("C:\\Users\\operator\\AppData\\Roaming"),
        );
        assert_eq!(xdg, Some(PathBuf::from("/custom/config/bass/environment")));
        let windows = default_path(
            ConfigurationLayout::Windows,
            None,
            Some("/home/operator"),
            Some("C:\\Users\\operator\\AppData\\Roaming"),
        );
        assert_eq!(
            windows,
            Some(
                PathBuf::from("C:\\Users\\operator\\AppData\\Roaming")
                    .join("bass")
                    .join("environment")
            )
        );
        assert_eq!(
            default_path(ConfigurationLayout::Unix, Some(""), Some(""), Some("")),
            None
        );
    }
}
