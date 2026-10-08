//! Dictionary metadata from `Info.plist` (compiled bundles) or the DDK
//! project's info plist; both use the same keys.

use std::path::Path;

use dictdb::DictInfo;
use plist::Dictionary;

use crate::error::{Error, Result};

pub(crate) fn read_plist(path: &Path) -> Result<Dictionary> {
    let value = plist::Value::from_file(path).map_err(|source| Error::Plist {
        path: path.to_owned(),
        source,
    })?;
    value.into_dictionary().ok_or_else(|| Error::Unsupported {
        path: path.to_owned(),
        reason: "plist root is not a dictionary".into(),
    })
}

pub(crate) fn dict_info(plist: &Dictionary, fallback_name: &str, source_kind: &str) -> DictInfo {
    let string = |key: &str| {
        plist
            .get(key)
            .and_then(|v| v.as_string())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let identifier = string("CFBundleIdentifier").unwrap_or_else(|| fallback_name.to_owned());
    let name = string("CFBundleDisplayName")
        .or_else(|| string("CFBundleName"))
        .unwrap_or_else(|| fallback_name.to_owned());

    let mut languages = Vec::new();
    for lang in plist
        .get("DCSDictionaryLanguages")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_dictionary())
    {
        for key in [
            "DCSDictionaryIndexLanguage",
            "DCSDictionaryDescriptionLanguage",
        ] {
            if let Some(code) = lang.get(key).and_then(|v| v.as_string())
                && !languages.iter().any(|l| l == code)
            {
                languages.push(code.to_owned());
            }
        }
    }

    DictInfo {
        name,
        identifier,
        languages,
        source_kind: source_kind.to_owned(),
        copyright: string("DCSDictionaryCopyright"),
    }
}
