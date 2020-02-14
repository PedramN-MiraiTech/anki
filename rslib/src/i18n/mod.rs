// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use fluent::{FluentArgs, FluentBundle, FluentResource};
use log::error;
use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use unic_langid::LanguageIdentifier;

pub use fluent::fluent_args as tr_args;

/// Helper for creating args with &strs
#[macro_export]
macro_rules! tr_strs {
    ( $($key:expr => $value:expr),* ) => {
        {
            let mut args: fluent::FluentArgs = fluent::FluentArgs::new();
            $(
                args.insert($key, $value.to_string().into());
            )*
            args
        }
    };
}
pub use tr_strs;

/// All languages we (currently) support, excluding the fallback
/// English.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum LanguageDialect {
    Japanese,
    ChineseMainland,
    ChineseTaiwan,
}

fn lang_dialect(lang: LanguageIdentifier) -> Option<LanguageDialect> {
    use LanguageDialect as L;
    Some(match lang.get_language() {
        "ja" => L::Japanese,
        "zh" => match lang.get_region() {
            Some("TW") => L::ChineseTaiwan,
            _ => L::ChineseMainland,
        },
        _ => return None,
    })
}

fn dialect_file_locale(dialect: LanguageDialect) -> &'static str {
    match dialect {
        LanguageDialect::Japanese => "ja",
        LanguageDialect::ChineseMainland => "zh",
        LanguageDialect::ChineseTaiwan => todo!(),
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TranslationFile {
    Test,
    MediaCheck,
}

fn data_for_fallback(file: TranslationFile) -> String {
    match file {
        TranslationFile::MediaCheck => include_str!("media-check.ftl"),
        TranslationFile::Test => include_str!("../../tests/support/test.ftl"),
    }
    .to_string()
}

fn data_for_lang_and_file(
    dialect: LanguageDialect,
    file: TranslationFile,
    locales: &Path,
) -> Option<String> {
    let path = locales.join(dialect_file_locale(dialect)).join(match file {
        TranslationFile::MediaCheck => "media-check.ftl",
        TranslationFile::Test => "test.ftl",
    });
    fs::read_to_string(&path)
        .map_err(|e| {
            error!("Unable to read translation file: {:?}: {}", path, e);
        })
        .ok()
}

fn get_bundle(
    text: String,
    locales: &[LanguageIdentifier],
) -> Option<FluentBundle<FluentResource>> {
    let res = FluentResource::try_new(text)
        .map_err(|e| {
            error!("Unable to parse translations file: {:?}", e);
        })
        .ok()?;

    let mut bundle: FluentBundle<FluentResource> = FluentBundle::new(locales);
    bundle
        .add_resource(res)
        .map_err(|e| {
            error!("Duplicate key detected in translation file: {:?}", e);
        })
        .ok()?;

    Some(bundle)
}

pub struct I18n {
    // language identifiers, used for date/time rendering
    langs: Vec<LanguageIdentifier>,
    // languages supported by us
    supported: Vec<LanguageDialect>,

    locale_folder: PathBuf,
}

impl I18n {
    pub fn new<S: AsRef<str>, P: Into<PathBuf>>(locale_codes: &[S], locale_folder: P) -> Self {
        let mut langs = vec![];
        let mut supported = vec![];
        for code in locale_codes {
            if let Ok(ident) = code.as_ref().parse::<LanguageIdentifier>() {
                langs.push(ident.clone());
                if let Some(dialect) = lang_dialect(ident) {
                    supported.push(dialect)
                }
            }
        }
        // add fallback date/time
        langs.push("en_US".parse().unwrap());

        Self {
            langs,
            supported,
            locale_folder: locale_folder.into(),
        }
    }

    pub fn get(&self, file: TranslationFile) -> I18nCategory {
        I18nCategory::new(&*self.langs, &*self.supported, file, &self.locale_folder)
    }
}

pub struct I18nCategory {
    // bundles in preferred language order, with fallback English as the
    // last element
    bundles: Vec<FluentBundle<FluentResource>>,
}

impl I18nCategory {
    pub fn new(
        langs: &[LanguageIdentifier],
        preferred: &[LanguageDialect],
        file: TranslationFile,
        locale_folder: &Path,
    ) -> Self {
        let mut bundles = Vec::with_capacity(preferred.len() + 1);
        for dialect in preferred {
            if let Some(text) = data_for_lang_and_file(*dialect, file, locale_folder) {
                if let Some(mut bundle) = get_bundle(text, langs) {
                    if cfg!(test) {
                        bundle.set_use_isolating(false);
                    }
                    bundles.push(bundle);
                } else {
                    error!("Failed to create bundle for {:?} {:?}", dialect, file);
                }
            }
        }

        let mut fallback_bundle = get_bundle(data_for_fallback(file), langs).unwrap();
        if cfg!(test) {
            fallback_bundle.set_use_isolating(false);
        }

        bundles.push(fallback_bundle);

        Self { bundles }
    }

    /// Get translation with zero arguments.
    pub fn tr(&self, key: &str) -> Cow<str> {
        self.tr_(key, None)
    }

    /// Get translation with one or more arguments.
    pub fn trn(&self, key: &str, args: FluentArgs) -> String {
        self.tr_(key, Some(args)).into()
    }

    fn tr_<'a>(&'a self, key: &str, args: Option<FluentArgs>) -> Cow<'a, str> {
        for bundle in &self.bundles {
            let msg = match bundle.get_message(key) {
                Some(msg) => msg,
                // not translated in this bundle
                None => continue,
            };

            let pat = match msg.value {
                Some(val) => val,
                // empty value
                None => continue,
            };

            let mut errs = vec![];
            let out = bundle.format_pattern(pat, args.as_ref(), &mut errs);
            if !errs.is_empty() {
                error!("Error(s) in translation '{}': {:?}", key, errs);
            }
            // clone so we can discard args
            return out.to_string().into();
        }

        format!("Missing translation key: {}", key).into()
    }
}

#[cfg(test)]
mod test {
    use crate::i18n::{dialect_file_locale, lang_dialect, TranslationFile};
    use crate::i18n::{tr_args, I18n, LanguageDialect};
    use std::path::PathBuf;
    use unic_langid::LanguageIdentifier;

    #[test]
    fn dialect() {
        use LanguageDialect as L;
        let mut ident: LanguageIdentifier = "en-US".parse().unwrap();
        assert_eq!(lang_dialect(ident), None);
        ident = "ja_JP".parse().unwrap();
        assert_eq!(lang_dialect(ident), Some(L::Japanese));
        ident = "zh".parse().unwrap();
        assert_eq!(lang_dialect(ident), Some(L::ChineseMainland));
        ident = "zh-TW".parse().unwrap();
        assert_eq!(lang_dialect(ident), Some(L::ChineseTaiwan));

        assert_eq!(dialect_file_locale(L::Japanese), "ja");
        assert_eq!(dialect_file_locale(L::ChineseMainland), "zh");
        //        assert_eq!(dialect_file_locale(L::Other), "templates");
    }

    #[test]
    fn i18n() {
        // English fallback
        let i18n = I18n::new(&["zz"], "../../tests/support");
        let cat = i18n.get(TranslationFile::Test);
        assert_eq!(cat.tr("valid-key"), "a valid key");
        assert_eq!(
            cat.tr("invalid-key"),
            "Missing translation key: invalid-key"
        );

        assert_eq!(
            cat.trn("two-args-key", tr_args!["one"=>1, "two"=>"2"]),
            "two args: 1 and 2"
        );

        // commented out to avoid scary warning during unit tests
        //        assert_eq!(
        //            cat.trn("two-args-key", tr_args!["one"=>"testing error reporting"]),
        //            "two args: testing error reporting and {$two}"
        //        );

        assert_eq!(cat.trn("plural", tr_args!["hats"=>1]), "You have 1 hat.");
        assert_eq!(cat.trn("plural", tr_args!["hats"=>3]), "You have 3 hats.");

        // Other language
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("tests/support");
        let i18n = I18n::new(&["ja_JP"], d);
        let cat = i18n.get(TranslationFile::Test);
        assert_eq!(cat.tr("valid-key"), "キー");
        assert_eq!(cat.tr("only-in-english"), "not translated");
        assert_eq!(
            cat.tr("invalid-key"),
            "Missing translation key: invalid-key"
        );

        assert_eq!(
            cat.trn("two-args-key", tr_args!["one"=>1, "two"=>"2"]),
            "1と2"
        );
    }
}
