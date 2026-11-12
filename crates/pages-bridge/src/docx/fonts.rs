//! Maps Pages' PostScript font names to font families installed on this machine, and knows the
//! line heights Pages lays its fonts out with.

use std::collections::{HashMap, HashSet};

/// Families whose PostScript names do not split into words at capitals.
const KNOWN: &[(&str, &str)] = &[
    ("helveticaneue", "Helvetica Neue"),
    ("timesnewroman", "Times New Roman"),
    ("couriernew", "Courier New"),
    ("trebuchetms", "Trebuchet MS"),
    ("comicsansms", "Comic Sans MS"),
    ("avenirnext", "Avenir Next"),
    ("avenirnextcondensed", "Avenir Next Condensed"),
    ("gillsans", "Gill Sans"),
    ("hoeflertext", "Hoefler Text"),
    ("americantypewriter", "American Typewriter"),
    ("markerfelt", "Marker Felt"),
    ("sfpro", "SF Pro"),
    ("sfprotext", "SF Pro Text"),
    ("sfprodisplay", "SF Pro Display"),
    ("sfmono", "SF Mono"),
    ("lucidagrande", "Lucida Grande"),
    ("chalkboardse", "Chalkboard SE"),
    ("bradleyhand", "Bradley Hand"),
    ("snellroundhand", "Snell Roundhand"),
    ("applechancery", "Apple Chancery"),
];

/// Windows stand-ins for Apple fonts, used only when the original family is not installed.
const SUBSTITUTES: &[(&str, &str)] = &[
    ("helvetica neue", "Arial"),
    ("helvetica", "Arial"),
    ("sf pro", "Segoe UI"),
    ("sf pro text", "Segoe UI"),
    ("sf pro display", "Segoe UI"),
    ("lucida grande", "Segoe UI"),
    ("avenir", "Segoe UI"),
    ("avenir next", "Segoe UI"),
    ("avenir next condensed", "Segoe UI"),
    ("seravek", "Segoe UI"),
    ("geneva", "Verdana"),
    ("futura", "Century Gothic"),
    ("gill sans", "Gill Sans MT"),
    ("optima", "Candara"),
    ("times", "Times New Roman"),
    ("hoefler text", "Georgia"),
    ("cochin", "Georgia"),
    ("baskerville", "Georgia"),
    ("big caslon", "Georgia"),
    ("charter", "Georgia"),
    ("iowan old style", "Georgia"),
    ("didot", "Bodoni MT"),
    ("palatino", "Palatino Linotype"),
    ("american typewriter", "Courier New"),
    ("courier", "Courier New"),
    ("menlo", "Consolas"),
    ("monaco", "Consolas"),
    ("sf mono", "Consolas"),
    ("marker felt", "Comic Sans MS"),
    ("chalkboard", "Comic Sans MS"),
    ("chalkboard se", "Comic Sans MS"),
    ("noteworthy", "Comic Sans MS"),
    ("chalkduster", "Comic Sans MS"),
    ("bradley hand", "Segoe Print"),
    ("snell roundhand", "Segoe Script"),
    ("zapfino", "Segoe Script"),
    ("apple chancery", "Segoe Script"),
];

/// Ascent and descent in em of a font as Pages lays it out.
struct Metrics {
    family: &'static str,
    regular: (f64, f64),
    bold: (f64, f64),
}

/// Fonts whose Pages layout has been measured against a Pages PDF export. Pages rounds ascent
/// and descent to whole points each, e.g. 12 pt Helvetica Neue gives 11 + 3 = 14 pt per line
/// and 10 pt gives 10 + 2. Bold lines come out taller, which the bold values reproduce.
const PAGES_METRICS: &[Metrics] = &[Metrics { family: "helvetica neue", regular: (0.952, 0.213), bold: (0.975, 0.20) }];

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FontInfo {
    /// Family to write into the document.
    pub family: String,
    pub implies_bold: bool,
    pub implies_italic: bool,
}

/// Pages' single line of a font at a size, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PagesLine {
    pub height: f64,
    pub descent: f64,
}

pub(crate) struct FontMapper {
    installed: Option<HashSet<String>>,
    cache: HashMap<String, (FontInfo, String)>,
}

impl FontMapper {
    pub(crate) fn new() -> Self {
        Self { installed: crate::system::installed_font_families(), cache: HashMap::new() }
    }

    #[cfg(test)]
    fn with_installed(installed: &[&str]) -> Self {
        Self { installed: Some(installed.iter().map(|s| s.to_lowercase()).collect()), cache: HashMap::new() }
    }

    fn is_installed(&self, family: &str) -> bool {
        self.installed.as_ref().is_none_or(|set| set.contains(&family.to_lowercase()))
    }

    /// Resolves a PostScript name such as `HelveticaNeue-BoldItalic`.
    pub(crate) fn resolve(&mut self, ps_name: &str) -> FontInfo {
        self.entry(ps_name).0.clone()
    }

    /// The family Pages itself used, before any substitution.
    fn original_family(&mut self, ps_name: &str) -> String {
        self.entry(ps_name).1.clone()
    }

    fn entry(&mut self, ps_name: &str) -> &(FontInfo, String) {
        if !self.cache.contains_key(ps_name) {
            let computed = self.compute(ps_name);
            self.cache.insert(ps_name.to_owned(), computed);
        }
        &self.cache[ps_name]
    }

    fn compute(&self, ps_name: &str) -> (FontInfo, String) {
        let (base, face) = if ps_name.contains(' ') { (ps_name, "") } else { ps_name.split_once('-').unwrap_or((ps_name, "")) };
        let trimmed = ["PSMT", "MT", "PS"].iter().find_map(|s| base.strip_suffix(s)).unwrap_or(base);
        let key = trimmed.to_lowercase();
        let original = KNOWN
            .iter()
            .find(|(k, _)| *k == key)
            .map_or_else(|| split_camel_case(trimmed), |(_, family)| (*family).to_owned());
        let implies_bold = ["Bold", "Heavy", "Black", "Semibold", "SemiBold", "Demi"].iter().any(|w| face.contains(w));
        let implies_italic = face.contains("Italic") || face.contains("Oblique");
        let weight: String = ["Bold", "Italic", "Oblique", "Regular", "Roman", "Book"]
            .iter()
            .fold(face.to_owned(), |acc, w| acc.replace(w, ""));
        let weighted = format!("{original} {weight}");
        let family = if !weight.is_empty() && self.installed.is_some() && self.is_installed(&weighted) {
            weighted
        } else if self.is_installed(&original) {
            original.clone()
        } else {
            let sub = SUBSTITUTES
                .iter()
                .find(|(k, _)| *k == original.to_lowercase())
                .map_or("Arial", |(_, s)| *s);
            if self.is_installed(sub) { sub.to_owned() } else { original.clone() }
        };
        (FontInfo { family, implies_bold, implies_italic }, original)
    }

    /// Pages' single line height for this font, when its layout has been measured.
    pub(crate) fn pages_line(&mut self, ps_name: Option<&str>, size: f64, bold: bool) -> Option<PagesLine> {
        let original = self.original_family(ps_name?).to_lowercase();
        let metrics = PAGES_METRICS.iter().find(|m| m.family == original)?;
        let (ascent, descent) = if bold { metrics.bold } else { metrics.regular };
        let descent = (descent * size).round();
        Some(PagesLine { height: (ascent * size).round() + descent, descent })
    }
}

fn split_camel_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_lowercase();
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_missing_apple_fonts() {
        let mut fonts = FontMapper::with_installed(&["Arial", "Georgia"]);
        let info = fonts.resolve("HelveticaNeue-Bold");
        assert_eq!((info.family.as_str(), info.implies_bold, info.implies_italic), ("Arial", true, false));
        assert_eq!(fonts.resolve("Georgia-Italic").family, "Georgia");
        assert_eq!(fonts.resolve("TimesNewRomanPSMT").family, "Arial");
    }

    #[test]
    fn keeps_installed_fonts() {
        let mut fonts = FontMapper::with_installed(&["Helvetica Neue"]);
        assert_eq!(fonts.resolve("HelveticaNeue").family, "Helvetica Neue");
    }

    #[test]
    fn pages_line_heights_match_measurements() {
        let mut fonts = FontMapper::with_installed(&["Arial"]);
        let line = fonts.pages_line(Some("HelveticaNeue"), 12.0, false).unwrap();
        assert_eq!((line.height, line.descent), (14.0, 3.0));
        assert_eq!(fonts.pages_line(Some("HelveticaNeue"), 10.0, false).unwrap().height, 12.0);
        assert_eq!(fonts.pages_line(Some("Georgia"), 12.0, false), None);
    }
}
