//! A language per office (issue #31): what its bots write customer-facing
//! text in. The owner picks one from this list in Edit office; the list is
//! served at `GET /api/languages` so the app and the server agree.
//!
//! TODO(air): a new office has no language until the owner picks one. A
//! default from the browser's language would save a step but could surprise
//! an owner whose customers speak another language than their computer.

use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

/// A language an office can work in.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Language {
    pub code: &'static str,
    /// Its name in English, for the bots' instructions.
    pub name: &'static str,
    /// Its name in itself, for the picker.
    pub native: &'static str,
}

const fn lang(code: &'static str, name: &'static str, native: &'static str) -> Language {
    Language { code, name, native }
}

pub const LANGUAGES: &[Language] = &[
    lang("ar", "Arabic", "العربية"),
    lang("bn", "Bengali", "বাংলা"),
    lang("cs", "Czech", "Čeština"),
    lang("da", "Danish", "Dansk"),
    lang("de", "German", "Deutsch"),
    lang("el", "Greek", "Ελληνικά"),
    lang("en", "English", "English"),
    lang("es", "Spanish", "Español"),
    lang("fa", "Persian", "فارسی"),
    lang("fi", "Finnish", "Suomi"),
    lang("fr", "French", "Français"),
    lang("he", "Hebrew", "עברית"),
    lang("hi", "Hindi", "हिन्दी"),
    lang("hu", "Hungarian", "Magyar"),
    lang("id", "Indonesian", "Bahasa Indonesia"),
    lang("it", "Italian", "Italiano"),
    lang("ja", "Japanese", "日本語"),
    lang("kk", "Kazakh", "Қазақ тілі"),
    lang("ko", "Korean", "한국어"),
    lang("ms", "Malay", "Bahasa Melayu"),
    lang("nl", "Dutch", "Nederlands"),
    lang("no", "Norwegian", "Norsk"),
    lang("pl", "Polish", "Polski"),
    lang("pt", "Portuguese", "Português"),
    lang("pt-BR", "Brazilian Portuguese", "Português (Brasil)"),
    lang("ro", "Romanian", "Română"),
    lang("ru", "Russian", "Русский"),
    lang("sv", "Swedish", "Svenska"),
    lang("sw", "Swahili", "Kiswahili"),
    lang("th", "Thai", "ไทย"),
    lang("tl", "Filipino", "Filipino"),
    lang("tr", "Turkish", "Türkçe"),
    lang("uk", "Ukrainian", "Українська"),
    lang("ur", "Urdu", "اردو"),
    lang("uz", "Uzbek", "Oʻzbekcha"),
    lang("vi", "Vietnamese", "Tiếng Việt"),
    lang("zh-CN", "Simplified Chinese", "简体中文"),
    lang("zh-TW", "Traditional Chinese", "繁體中文"),
];

/// The language with this code, if offered.
pub fn find(code: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.code == code)
}

/// What an office's bots are told about its language.
pub fn prompt_block(code: Option<&str>) -> String {
    let Some(lang) = code.and_then(find) else {
        return String::new();
    };
    let name = if lang.name == lang.native {
        lang.name.to_string()
    } else {
        format!("{} ({})", lang.name, lang.native)
    };
    format!(
        "\n## Language\nThis office works in {name}. Write everything customers or the public will see in {short}: \
         posts, emails, messages, quotes, invoices, documents and replies to customers. \
         If a customer writes in another language, answer them in theirs. \
         When the owner or a teammate writes to you in another language, answer them in that language, but keep drafts meant for customers in {short}.\n",
        short = lang.name,
    )
}

/// GET /api/languages: the languages an office can work in.
pub async fn list_languages() -> Json<Value> {
    Json(json!({ "languages": LANGUAGES }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_fit_the_column() {
        for (i, l) in LANGUAGES.iter().enumerate() {
            assert!(
                LANGUAGES[i + 1..].iter().all(|o| o.code != l.code),
                "{} twice",
                l.code
            );
            let (base, region) = l.code.split_once('-').unwrap_or((l.code, ""));
            assert!((2..=3).contains(&base.len()) && base.chars().all(|c| c.is_ascii_lowercase()));
            assert!(
                region.is_empty()
                    || (region.len() == 2 && region.chars().all(|c| c.is_ascii_uppercase()))
            );
        }
    }

    #[test]
    fn the_block_names_the_language() {
        assert_eq!(prompt_block(None), "");
        assert_eq!(prompt_block(Some("xx")), "");
        let es = prompt_block(Some("es"));
        assert!(
            es.contains("This office works in Spanish (Español)."),
            "{es}"
        );
        assert!(
            es.contains("keep drafts meant for customers in Spanish."),
            "{es}"
        );
        assert!(prompt_block(Some("en")).contains("works in English."));
    }
}
