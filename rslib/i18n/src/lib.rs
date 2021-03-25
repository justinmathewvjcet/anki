// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

mod generated;

use fluent::{concurrent::FluentBundle, FluentArgs, FluentResource, FluentValue};
use num_format::Locale;
use serde::Serialize;
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use unic_langid::LanguageIdentifier;

use generated::{KEYS_BY_MODULE, STRINGS};

pub use generated::LegacyKey as TR;

pub use fluent::fluent_args as tr_args;

/// Helper for creating args with &strs
#[macro_export]
macro_rules! tr_strs {
    ( $($key:expr => $value:expr),* ) => {
        {
            let mut args: fluent::FluentArgs = fluent::FluentArgs::new();
            $(
                args.add($key, $value.to_string().into());
            )*
            args
        }
    };
}

fn remapped_lang_name(lang: &LanguageIdentifier) -> &str {
    let region = match &lang.region {
        Some(region) => Some(region.as_str()),
        None => None,
    };
    match lang.language.as_str() {
        "en" => {
            match region {
                Some("GB") | Some("AU") => "en-GB",
                // go directly to fallback
                _ => "templates",
            }
        }
        "zh" => match region {
            Some("TW") | Some("HK") => "zh-TW",
            _ => "zh-CN",
        },
        "pt" => {
            if let Some("PT") = region {
                "pt-PT"
            } else {
                "pt-BR"
            }
        }
        "ga" => "ga-IE",
        "hy" => "hy-AM",
        "nb" => "nb-NO",
        "sv" => "sv-SE",
        other => other,
    }
}

/// Some sample text for testing purposes.
fn test_en_text() -> &'static str {
    "
valid-key = a valid key
only-in-english = not translated
two-args-key = two args: {$one} and {$two}
plural = You have {$hats ->
    [one]   1 hat
   *[other] {$hats} hats
}.
"
}

fn test_jp_text() -> &'static str {
    "
valid-key = キー
two-args-key = {$one}と{$two}    
"
}

fn test_pl_text() -> &'static str {
    "
one-arg-key = fake Polish {$one}
"
}

/// Parse resource text into an AST for inclusion in a bundle.
/// Returns None if text contains errors.
/// extra_text may contain resources loaded from the filesystem
/// at runtime. If it contains errors, they will not prevent a
/// bundle from being returned.
fn get_bundle(
    text: &str,
    extra_text: String,
    locales: &[LanguageIdentifier],
) -> Option<FluentBundle<FluentResource>> {
    let res = FluentResource::try_new(text.into())
        .map_err(|e| {
            println!("Unable to parse translations file: {:?}", e);
        })
        .ok()?;

    let mut bundle: FluentBundle<FluentResource> = FluentBundle::new(locales);
    bundle
        .add_resource(res)
        .map_err(|e| {
            println!("Duplicate key detected in translation file: {:?}", e);
        })
        .ok()?;

    if !extra_text.is_empty() {
        match FluentResource::try_new(extra_text) {
            Ok(res) => bundle.add_resource_overriding(res),
            Err((_res, e)) => println!("Unable to parse translations file: {:?}", e),
        }
    }

    // add numeric formatter
    set_bundle_formatter_for_langs(&mut bundle, locales);

    Some(bundle)
}

/// Get a bundle that includes any filesystem overrides.
fn get_bundle_with_extra(
    text: &str,
    lang: Option<LanguageIdentifier>,
) -> Option<FluentBundle<FluentResource>> {
    let mut extra_text = "".into();
    if cfg!(test) {
        // inject some test strings in test mode
        match &lang {
            None => {
                extra_text += test_en_text();
            }
            Some(lang) if lang.language == "ja" => {
                extra_text += test_jp_text();
            }
            Some(lang) if lang.language == "pl" => {
                extra_text += test_pl_text();
            }
            _ => {}
        }
    }

    let mut locales = if let Some(lang) = lang {
        vec![lang]
    } else {
        vec![]
    };
    locales.push("en-US".parse().unwrap());

    get_bundle(text, extra_text, &locales)
}

#[derive(Clone)]
pub struct I18n {
    inner: Arc<Mutex<I18nInner>>,
}

fn get_key_legacy(val: usize) -> &'static str {
    let (module_idx, translation_idx) = (val / 1000, val % 1000);
    get_key(module_idx, translation_idx)
}

fn get_key(module_idx: usize, translation_idx: usize) -> &'static str {
    KEYS_BY_MODULE
        .get(module_idx)
        .and_then(|translations| translations.get(translation_idx))
        .cloned()
        .unwrap_or("invalid-module-or-translation-index")
}

impl I18n {
    pub fn template_only() -> Self {
        Self::new::<&str>(&[])
    }

    pub fn new<S: AsRef<str>>(locale_codes: &[S]) -> Self {
        let mut input_langs = vec![];
        let mut bundles = Vec::with_capacity(locale_codes.len() + 1);
        let mut resource_text = vec![];

        for code in locale_codes {
            let code = code.as_ref();
            if let Ok(lang) = code.parse::<LanguageIdentifier>() {
                input_langs.push(lang.clone());
                if lang.language == "en" {
                    // if English was listed, any further preferences are skipped,
                    // as the template has 100% coverage, and we need to ensure
                    // it is tried prior to any other langs.
                    break;
                }
            }
        }

        let mut output_langs = vec![];
        for lang in input_langs {
            // if the language is bundled in the binary
            if let Some(text) = ftl_localized_text(&lang).or_else(|| {
                // when testing, allow missing translations
                if cfg!(test) {
                    Some(String::new())
                } else {
                    None
                }
            }) {
                if let Some(bundle) = get_bundle_with_extra(&text, Some(lang.clone())) {
                    resource_text.push(text);
                    bundles.push(bundle);
                    output_langs.push(lang);
                } else {
                    println!("Failed to create bundle for {:?}", lang.language)
                }
            }
        }

        // add English templates
        let template_lang = "en-US".parse().unwrap();
        let template_text = ftl_localized_text(&template_lang).unwrap();
        let template_bundle = get_bundle_with_extra(&template_text, None).unwrap();
        resource_text.push(template_text);
        bundles.push(template_bundle);
        output_langs.push(template_lang);

        if locale_codes.is_empty() || cfg!(test) {
            // disable isolation characters in test mode
            for bundle in &mut bundles {
                bundle.set_use_isolating(false);
            }
        }

        Self {
            inner: Arc::new(Mutex::new(I18nInner {
                bundles,
                langs: output_langs,
                resource_text,
            })),
        }
    }

    /// Get translation with zero arguments.
    pub fn tr(&self, key: TR) -> Cow<str> {
        let key = get_key_legacy(key as usize);
        self.tr_(key, None)
    }

    /// Get translation with one or more arguments.
    pub fn trn(&self, key: TR, args: FluentArgs) -> String {
        let key = get_key_legacy(key as usize);
        self.tr_(key, Some(args)).into()
    }

    pub fn trn2(&self, key: usize, args: FluentArgs) -> String {
        let key = get_key_legacy(key);
        self.tr_(key, Some(args)).into()
    }

    fn tr_<'a>(&'a self, key: &str, args: Option<FluentArgs>) -> Cow<'a, str> {
        for bundle in &self.inner.lock().unwrap().bundles {
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
                println!("Error(s) in translation '{}': {:?}", key, errs);
            }
            // clone so we can discard args
            return out.to_string().into();
        }

        // return the key name if it was missing
        key.to_string().into()
    }

    /// Return text from configured locales for use with the JS Fluent implementation.
    pub fn resources_for_js(&self) -> ResourcesForJavascript {
        let inner = self.inner.lock().unwrap();
        ResourcesForJavascript {
            langs: inner.langs.iter().map(ToString::to_string).collect(),
            resources: inner.resource_text.clone(),
        }
    }
}

/// This temporarily behaves like the older code; in the future we could either
/// access each &str separately, or load them on demand.
fn ftl_localized_text(lang: &LanguageIdentifier) -> Option<String> {
    let lang = remapped_lang_name(lang);
    if let Some(module) = STRINGS.get(lang) {
        let mut text = String::new();
        for module_text in module.values() {
            text.push_str(module_text)
        }
        Some(text)
    } else {
        None
    }
}

struct I18nInner {
    // bundles in preferred language order, with template English as the
    // last element
    bundles: Vec<FluentBundle<FluentResource>>,
    langs: Vec<LanguageIdentifier>,
    // fixme: this is a relic from the old implementation, and we could gather
    // it only when needed in the future
    resource_text: Vec<String>,
}

// Simple number formatting implementation

fn set_bundle_formatter_for_langs<T>(bundle: &mut FluentBundle<T>, langs: &[LanguageIdentifier]) {
    let formatter = if want_comma_as_decimal_separator(langs) {
        format_decimal_with_comma
    } else {
        format_decimal_with_period
    };

    bundle.set_formatter(Some(formatter));
}

fn first_available_num_format_locale(langs: &[LanguageIdentifier]) -> Option<Locale> {
    for lang in langs {
        if let Some(locale) = num_format_locale(lang) {
            return Some(locale);
        }
    }
    None
}

// try to locate a num_format locale for a given language identifier
fn num_format_locale(lang: &LanguageIdentifier) -> Option<Locale> {
    // region provided?
    if let Some(region) = lang.region {
        let code = format!("{}_{}", lang.language, region);
        if let Ok(locale) = Locale::from_name(code) {
            return Some(locale);
        }
    }
    // try the language alone
    Locale::from_name(lang.language.as_str()).ok()
}

fn want_comma_as_decimal_separator(langs: &[LanguageIdentifier]) -> bool {
    let separator = if let Some(locale) = first_available_num_format_locale(langs) {
        locale.decimal()
    } else {
        "."
    };

    separator == ","
}

fn format_decimal_with_comma(
    val: &fluent::FluentValue,
    _intl: &intl_memoizer::concurrent::IntlLangMemoizer,
) -> Option<String> {
    format_number_values(val, Some(","))
}

fn format_decimal_with_period(
    val: &fluent::FluentValue,
    _intl: &intl_memoizer::concurrent::IntlLangMemoizer,
) -> Option<String> {
    format_number_values(val, None)
}

#[inline]
fn format_number_values(
    val: &fluent::FluentValue,
    alt_separator: Option<&'static str>,
) -> Option<String> {
    match val {
        FluentValue::Number(num) => {
            // create a string with desired maximum digits
            let max_frac_digits = 2;
            let with_max_precision = format!(
                "{number:.precision$}",
                number = num.value,
                precision = max_frac_digits
            );

            // remove any excess trailing zeros
            let mut val: Cow<str> = with_max_precision.trim_end_matches('0').into();

            // adding back any required to meet minimum_fraction_digits
            if let Some(minfd) = num.options.minimum_fraction_digits {
                let pos = val.find('.').expect("expected . in formatted string");
                let frac_num = val.len() - pos - 1;
                let zeros_needed = minfd - frac_num;
                if zeros_needed > 0 {
                    val = format!("{}{}", val, "0".repeat(zeros_needed)).into();
                }
            }

            // lop off any trailing '.'
            let result = val.trim_end_matches('.');

            if let Some(sep) = alt_separator {
                Some(result.replace('.', sep))
            } else {
                Some(result.to_string())
            }
        }
        _ => None,
    }
}

#[derive(Serialize)]
pub struct ResourcesForJavascript {
    langs: Vec<String>,
    resources: Vec<String>,
}

#[cfg(test)]
mod test {
    use super::*;
    use unic_langid::langid;

    #[test]
    fn numbers() {
        assert_eq!(want_comma_as_decimal_separator(&[langid!("en-US")]), false);
        assert_eq!(want_comma_as_decimal_separator(&[langid!("pl-PL")]), true);
    }

    #[test]
    fn i18n() {
        // English template
        let i18n = I18n::new(&["zz"]);
        assert_eq!(i18n.tr_("valid-key", None), "a valid key");
        assert_eq!(i18n.tr_("invalid-key", None), "invalid-key");

        assert_eq!(
            i18n.tr_("two-args-key", Some(tr_args!["one"=>1.1, "two"=>"2"])),
            "two args: 1.1 and 2"
        );

        assert_eq!(
            i18n.tr_("plural", Some(tr_args!["hats"=>1.0])),
            "You have 1 hat."
        );
        assert_eq!(
            i18n.tr_("plural", Some(tr_args!["hats"=>1.1])),
            "You have 1.1 hats."
        );
        assert_eq!(
            i18n.tr_("plural", Some(tr_args!["hats"=>3])),
            "You have 3 hats."
        );

        // Another language
        let i18n = I18n::new(&["ja_JP"]);
        assert_eq!(i18n.tr_("valid-key", None), "キー");
        assert_eq!(i18n.tr_("only-in-english", None), "not translated");
        assert_eq!(i18n.tr_("invalid-key", None), "invalid-key");

        assert_eq!(
            i18n.tr_("two-args-key", Some(tr_args!["one"=>1, "two"=>"2"])),
            "1と2"
        );

        // Decimal separator
        let i18n = I18n::new(&["pl-PL"]);
        // Polish will use a comma if the string is translated
        assert_eq!(
            i18n.tr_("one-arg-key", Some(tr_args!["one"=>2.07])),
            "fake Polish 2,07"
        );

        // but if it falls back on English, it will use an English separator
        assert_eq!(
            i18n.tr_("two-args-key", Some(tr_args!["one"=>1, "two"=>2.07])),
            "two args: 1 and 2.07"
        );
    }
}
