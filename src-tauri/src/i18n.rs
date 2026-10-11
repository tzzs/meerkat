//! The few words Pulse says outside the webview: toasts, the tray menu and the tray tooltips.
//! The front end keeps its own catalogue (`src/i18n`); these strings follow its wording, and
//! the unit formatting below mirrors `src/lib/format.ts` so a figure reads the same in a toast,
//! a tooltip and the window.

use crate::models::{Insight, InsightKind, MetricKind};
use crate::settings::{Language, NetworkUnit, Settings, StorageUnit, TemperatureUnit};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    ZhHans,
}

/// `Auto` follows the Windows display language, the same signal the webview's own
/// `navigator.language` is derived from.
pub fn resolve(language: Language) -> Lang {
    match language {
        Language::En => Lang::En,
        Language::ZhHans => Lang::ZhHans,
        Language::Auto => from_lang_id(unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() }),
    }
}

/// A LANGID keeps the primary language in its low ten bits; 0x04 is Chinese, which has only
/// the Simplified catalogue so far.
pub fn from_lang_id(id: u16) -> Lang {
    if id & 0x3ff == 0x04 {
        Lang::ZhHans
    } else {
        Lang::En
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Text {
    OpenPulse,
    Settings,
    QuitPulse,
    /// The product name, which is also the tray icon's tooltip when no readout is enabled.
    Pulse,
    /// After a share of memory: "38% available".
    Available,
    /// The disk tooltip's "Reading 12 MB/s, Writing 4 MB/s".
    Reading,
    Writing,
}

pub fn text(lang: Lang, text: Text) -> &'static str {
    match (lang, text) {
        (Lang::En, Text::OpenPulse) => "Open Pulse",
        (Lang::En, Text::Settings) => "Settings",
        (Lang::En, Text::QuitPulse) => "Quit Pulse",
        (Lang::En, Text::Pulse) | (Lang::ZhHans, Text::Pulse) => "Pulse",
        (Lang::En, Text::Available) => "available",
        (Lang::En, Text::Reading) => "Reading",
        (Lang::En, Text::Writing) => "Writing",
        (Lang::ZhHans, Text::OpenPulse) => "打开 Pulse",
        (Lang::ZhHans, Text::Settings) => "设置",
        (Lang::ZhHans, Text::QuitPulse) => "退出 Pulse",
        (Lang::ZhHans, Text::Available) => "可用",
        (Lang::ZhHans, Text::Reading) => "读取",
        (Lang::ZhHans, Text::Writing) => "写入",
    }
}

pub fn metric(lang: Lang, metric: MetricKind) -> &'static str {
    match (lang, metric) {
        (Lang::En, MetricKind::Cpu) => "CPU",
        (Lang::En, MetricKind::Memory) => "Memory",
        (Lang::En, MetricKind::Disk) => "Disk",
        (Lang::En, MetricKind::Network) => "Network",
        (Lang::En, MetricKind::Gpu) => "GPU",
        (Lang::En, MetricKind::Battery) => "Battery",
        (Lang::En, MetricKind::Temperature) => "Temperature",
        (Lang::ZhHans, MetricKind::Cpu) => "处理器",
        (Lang::ZhHans, MetricKind::Memory) => "内存",
        (Lang::ZhHans, MetricKind::Disk) => "磁盘",
        (Lang::ZhHans, MetricKind::Network) => "网络",
        (Lang::ZhHans, MetricKind::Gpu) => "GPU",
        (Lang::ZhHans, MetricKind::Battery) => "电池",
        (Lang::ZhHans, MetricKind::Temperature) => "温度",
    }
}

/// The unit choices from Settings that change how a figure is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Units {
    pub storage: StorageUnit,
    pub network: NetworkUnit,
    pub temperature: TemperatureUnit,
}

impl Units {
    pub fn of(settings: &Settings) -> Self {
        Self {
            storage: settings.storage_unit,
            network: settings.network_unit,
            temperature: settings.temperature_unit,
        }
    }
}

impl Default for Units {
    fn default() -> Self {
        Self {
            storage: StorageUnit::Decimal,
            network: NetworkUnit::Bytes,
            temperature: TemperatureUnit::Celsius,
        }
    }
}

const BINARY: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
const DECIMAL: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
const BITS: [&str; 4] = ["bit/s", "kbit/s", "Mbit/s", "Gbit/s"];
/// format.ts joins number and unit with a no-break space, so a wrapped tooltip never strands
/// the unit on its own line.
const NBSP: char = '\u{a0}';

/// How many steps of `step` fit under `magnitude`, capped at the last unit.
fn scale_for(magnitude: f64, step: f64, units: usize) -> (usize, f64) {
    let mut index = 0;
    let mut value = magnitude;
    while value >= step && index < units - 1 {
        value /= step;
        index += 1;
    }
    (index, step.powi(index as i32))
}

/// Three significant figures, as format.ts writes them.
fn digits(value: f64) -> usize {
    let value = value.abs();
    if value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    }
}

/// `Intl.NumberFormat("en-US")` for the fixed digit counts used here: grouped thousands.
fn number(value: f64, digits: usize) -> String {
    // Rounded half away from zero first, as Intl does: Rust's own formatting rounds a tie to
    // even and would print 62.5 as "62" next to a label that rounded it to 63.
    let scale = 10f64.powi(digits as i32);
    let rounded = (value * scale).round() / scale;
    let raw = format!("{:.*}", digits, rounded);
    let (sign, unsigned) = match raw.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", raw.as_str()),
    };
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (unsigned, None),
    };
    let mut grouped = String::with_capacity(raw.len() + 4);
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

/// `formatBytes`: decimal (KB = 1000) or binary (KiB = 1024) as the user chose.
pub fn bytes(value: f64, units: Units) -> String {
    let (step, labels) = match units.storage {
        StorageUnit::Binary => (1024.0, &BINARY),
        StorageUnit::Decimal => (1000.0, &DECIMAL),
    };
    let (index, divisor) = scale_for(value.abs(), step, labels.len());
    let scaled = value / divisor;
    format!("{}{NBSP}{}", number(scaled, digits(scaled)), labels[index])
}

/// `formatRate`: disk rates follow the storage unit; network rates can be bits instead,
/// which are always decimal steps.
pub fn rate(bytes_per_second: f64, units: Units, network: bool) -> String {
    if network && units.network == NetworkUnit::Bits {
        let bits = bytes_per_second * 8.0;
        let (index, divisor) = scale_for(bits.abs(), 1000.0, BITS.len());
        let scaled = bits / divisor;
        return format!("{}{NBSP}{}", number(scaled, digits(scaled)), BITS[index]);
    }
    format!("{}/s", bytes(bytes_per_second, units))
}

/// `formatTemp`.
pub fn temperature(celsius: f64, units: Units) -> String {
    let (value, unit) = match units.temperature {
        TemperatureUnit::Fahrenheit => (celsius * 1.8 + 32.0, 'F'),
        TemperatureUnit::Celsius => (celsius, 'C'),
    };
    format!("{}\u{b0}{unit}", number(value, 0))
}

pub fn percent(value: f64) -> String {
    format!("{}%", number(value, 0))
}

/// A span of time as a sentence uses it: "45 seconds", "15 minutes", "1 hour 5 minutes".
pub fn duration(seconds: u64, lang: Lang) -> String {
    fn plural(n: u64, one: &str, many: &str) -> String {
        format!("{n} {}", if n == 1 { one } else { many })
    }
    if seconds < 60 {
        return match lang {
            Lang::En => plural(seconds, "second", "seconds"),
            Lang::ZhHans => format!("{seconds} 秒"),
        };
    }
    let mut minutes = (seconds + 30) / 60;
    if minutes < 60 {
        return match lang {
            Lang::En => plural(minutes, "minute", "minutes"),
            Lang::ZhHans => format!("{minutes} 分钟"),
        };
    }
    let hours = minutes / 60;
    minutes %= 60;
    match (lang, minutes) {
        (Lang::En, 0) => plural(hours, "hour", "hours"),
        (Lang::En, _) => format!("{} {}", plural(hours, "hour", "hours"), plural(minutes, "minute", "minutes")),
        (Lang::ZhHans, 0) => format!("{hours} 小时"),
        (Lang::ZhHans, _) => format!("{hours} 小时 {minutes} 分钟"),
    }
}

/// The toast's headline is the application itself; Pulse's own name is already on the toast,
/// filed under its AppUserModelID.
pub fn insight_title(insight: &Insight) -> String {
    let name = insight.app_name.trim();
    if name.is_empty() {
        "Pulse".to_string()
    } else {
        name.to_string()
    }
}

/// What happened, in the units the user reads everywhere else. The figures follow the insight
/// contract: HighCpu is a share of the whole machine; GrowingMemory is the growth in bytes over
/// `sustained_seconds` (a measured span, so nothing here claims it is still climbing); the
/// disk and network rules are bytes per second held that long; Heat is the hottest sensor.
pub fn insight_body(insight: &Insight, lang: Lang, units: Units) -> String {
    let span = duration(insight.sustained_seconds, lang);
    let value = insight.value;
    match (insight.kind, lang) {
        (InsightKind::HighCpu, Lang::En) => format!("Using {} of the whole CPU for {span}", percent(value)),
        (InsightKind::HighCpu, Lang::ZhHans) => format!("持续 {span}占用整机 CPU 的 {}", percent(value)),
        (InsightKind::GrowingMemory, Lang::En) => format!("Memory grew by {} in the last {span}", bytes(value, units)),
        (InsightKind::GrowingMemory, Lang::ZhHans) => format!("内存在过去 {span}内增长了 {}", bytes(value, units)),
        (InsightKind::HeavyDisk, Lang::En) => {
            format!("Disk reads and writes at {} for {span}", rate(value, units, false))
        }
        (InsightKind::HeavyDisk, Lang::ZhHans) => format!("持续 {span}以 {} 读写磁盘", rate(value, units, false)),
        (InsightKind::HeavyNetwork, Lang::En) => format!(
            "Network traffic at {} for {span}; this app has the most connections",
            rate(value, units, true)
        ),
        (InsightKind::HeavyNetwork, Lang::ZhHans) => {
            format!("网络流量持续 {span}达到 {}，此应用的连接最多", rate(value, units, true))
        }
        (InsightKind::Heat, Lang::En) => format!(
            "The hottest sensor reached {}; this app is using the most CPU",
            temperature(value, units)
        ),
        (InsightKind::Heat, Lang::ZhHans) => {
            format!("最高温度达到 {}，此应用占用的 CPU 最多", temperature(value, units))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insight(kind: InsightKind, value: f64, sustained_seconds: u64) -> Insight {
        Insight {
            id: "x".into(),
            at_ms: 0,
            kind,
            app_key: "code".into(),
            app_name: "Visual Studio Code".into(),
            value,
            sustained_seconds,
            repeat_count: 0,
            day_key: "0".into(),
            resolved: false,
        }
    }

    fn all_kinds() -> Vec<Insight> {
        vec![
            insight(InsightKind::HighCpu, 92.4, 45),
            insight(InsightKind::GrowingMemory, 1_951.0 * 1_048_576.0, 900),
            insight(InsightKind::HeavyDisk, 45_200_000.0, 30),
            insight(InsightKind::HeavyNetwork, 25_100_000.0, 30),
            insight(InsightKind::Heat, 95.0, 60),
        ]
    }

    #[test]
    fn english_toast_bodies_say_what_happened_in_the_users_units() {
        let units = Units::default();
        let bodies: Vec<String> = all_kinds().iter().map(|i| insight_body(i, Lang::En, units)).collect();
        assert_eq!(bodies[0], "Using 92% of the whole CPU for 45 seconds");
        assert_eq!(bodies[1], "Memory grew by 2.05\u{a0}GB in the last 15 minutes");
        assert_eq!(bodies[2], "Disk reads and writes at 45.2\u{a0}MB/s for 30 seconds");
        assert_eq!(bodies[3], "Network traffic at 25.1\u{a0}MB/s for 30 seconds; this app has the most connections");
        assert_eq!(bodies[4], "The hottest sensor reached 95\u{b0}C; this app is using the most CPU");
    }

    #[test]
    fn chinese_toast_bodies_are_complete_sentences_too() {
        let units = Units::default();
        let bodies: Vec<String> = all_kinds().iter().map(|i| insight_body(i, Lang::ZhHans, units)).collect();
        assert_eq!(bodies[0], "持续 45 秒占用整机 CPU 的 92%");
        assert_eq!(bodies[1], "内存在过去 15 分钟内增长了 2.05\u{a0}GB");
        assert_eq!(bodies[2], "持续 30 秒以 45.2\u{a0}MB/s 读写磁盘");
        assert_eq!(bodies[3], "网络流量持续 30 秒达到 25.1\u{a0}MB/s，此应用的连接最多");
        assert_eq!(bodies[4], "最高温度达到 95\u{b0}C，此应用占用的 CPU 最多");
    }

    // The old wording promised the growth was still going on, which the measurement never knew,
    // and repeated the app name the headline already carries.
    #[test]
    fn no_toast_claims_more_than_was_measured_or_repeats_the_name() {
        for lang in [Lang::En, Lang::ZhHans] {
            for i in all_kinds() {
                let body = insight_body(&i, lang, Units::default());
                assert!(!body.to_lowercase().contains("climbing"), "{body}");
                assert!(!body.contains("仍在"), "{body}");
                assert!(!body.contains("Visual Studio Code"), "{body}");
                assert!(!body.contains("Pulse"), "{body}");
            }
        }
        assert_eq!(insight_title(&all_kinds()[0]), "Visual Studio Code");
        let mut nameless = all_kinds().remove(0);
        nameless.app_name = "  ".into();
        assert_eq!(insight_title(&nameless), "Pulse");
    }

    #[test]
    fn toast_figures_follow_the_unit_settings() {
        let units = Units {
            storage: StorageUnit::Binary,
            network: NetworkUnit::Bits,
            temperature: TemperatureUnit::Fahrenheit,
        };
        let memory = insight(InsightKind::GrowingMemory, 1_951.0 * 1_048_576.0, 1_800);
        assert_eq!(insight_body(&memory, Lang::En, units), "Memory grew by 1.91\u{a0}GiB in the last 30 minutes");
        let network = insight(InsightKind::HeavyNetwork, 25_100_000.0, 3_900);
        assert_eq!(
            insight_body(&network, Lang::En, units),
            "Network traffic at 201\u{a0}Mbit/s for 1 hour 5 minutes; this app has the most connections"
        );
        let heat = insight(InsightKind::Heat, 95.0, 60);
        assert!(insight_body(&heat, Lang::En, units).contains("203\u{b0}F"));
    }

    #[test]
    fn figures_are_written_the_way_format_ts_writes_them() {
        let decimal = Units::default();
        let binary = Units { storage: StorageUnit::Binary, ..decimal };
        assert_eq!(bytes(0.0, decimal), "0.00\u{a0}B");
        assert_eq!(bytes(999.0, decimal), "999\u{a0}B");
        assert_eq!(bytes(1_500.0, decimal), "1.50\u{a0}KB");
        assert_eq!(bytes(1_023.0 * 1024.0, binary), "1,023\u{a0}KiB");
        assert_eq!(rate(12_345.0, decimal, false), "12.3\u{a0}KB/s");
        let bits = Units { network: NetworkUnit::Bits, ..decimal };
        assert_eq!(rate(1_000.0, bits, true), "8.00\u{a0}kbit/s");
        assert_eq!(rate(1_000.0, bits, false), "1.00\u{a0}KB/s", "a disk rate is never in bits");
        assert_eq!(percent(16.4), "16%");
        assert_eq!(number(1_234_567.0, 0), "1,234,567");
        assert_eq!(number(-1_234.5, 1), "-1,234.5");
    }

    #[test]
    fn durations_read_as_words() {
        assert_eq!(duration(1, Lang::En), "1 second");
        assert_eq!(duration(59, Lang::En), "59 seconds");
        assert_eq!(duration(60, Lang::En), "1 minute");
        assert_eq!(duration(3_599, Lang::En), "1 hour");
        assert_eq!(duration(7_200, Lang::En), "2 hours");
        assert_eq!(duration(3_660, Lang::ZhHans), "1 小时 1 分钟");
        assert_eq!(duration(30, Lang::ZhHans), "30 秒");
    }

    #[test]
    fn chinese_is_recognised_from_the_primary_language() {
        assert_eq!(from_lang_id(0x0804), Lang::ZhHans, "zh-CN");
        assert_eq!(from_lang_id(0x0404), Lang::ZhHans, "zh-TW shares the primary id");
        assert_eq!(from_lang_id(0x0409), Lang::En, "en-US");
        assert_eq!(from_lang_id(0x0411), Lang::En, "ja-JP has no catalogue yet");
        assert_eq!(resolve(Language::ZhHans), Lang::ZhHans);
        assert_eq!(resolve(Language::En), Lang::En);
    }
}
