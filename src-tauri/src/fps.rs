//! PresentMon is the only FPS source. A missing or stale CSV is reported as no measurement,
//! never as a number derived from refresh rate or GPU utilisation.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::models::FpsSource;

const FILE_NAME: &str = "presentmon.csv";
const COLUMN: &str = "MsBetweenPresents";
const MAX_AGE: Duration = Duration::from_secs(5);

/// FPS from the newest presentation in PresentMon's log, if it is still current.
pub fn last_fps(data_dir: &Path) -> (Option<f64>, FpsSource) {
    let path = data_dir.join(FILE_NAME);
    let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) else {
        return (None, FpsSource::None);
    };
    if !SystemTime::now().duration_since(modified).is_ok_and(|age| age <= MAX_AGE) {
        return (None, FpsSource::None);
    }
    match read_last_fps(&path).ok().flatten() {
        Some(fps) => (Some(fps), FpsSource::PresentMon),
        None => (None, FpsSource::None),
    }
}

fn read_last_fps(path: &Path) -> std::io::Result<Option<f64>> {
    let mut file = File::open(path)?;
    let mut header = String::new();
    {
        let mut reader = BufReader::new(&file);
        if reader.read_line(&mut header)? == 0 {
            return Ok(None);
        }
    }
    Ok(fps_from_header_and_row(&header, &last_line(&mut file)?))
}

/// The last CSV record without reading the whole log, which PresentMon appends to for hours.
fn last_line(file: &mut File) -> std::io::Result<String> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(String::new());
    }
    let mut one = [0u8; 1];
    file.seek(SeekFrom::End(-1))?;
    file.read_exact(&mut one)?;
    let mut end = if one[0] == b'\n' { len - 1 } else { len };
    if end == 0 {
        return Ok(String::new());
    }

    let mut tail = Vec::new();
    while end > 0 {
        let take = end.min(4096);
        let start = end - take;
        file.seek(SeekFrom::Start(start))?;
        let mut chunk = vec![0u8; take as usize];
        file.read_exact(&mut chunk)?;
        if let Some(index) = chunk.iter().rposition(|byte| *byte == b'\n') {
            tail.extend_from_slice(&chunk[index + 1..]);
            return Ok(String::from_utf8_lossy(&tail).into_owned());
        }
        let mut next = chunk;
        next.extend_from_slice(&tail);
        tail = next;
        end = start;
    }
    Ok(String::from_utf8_lossy(&tail).into_owned())
}

/// Parses one CSV record. PresentMon quotes fields that can contain commas.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.trim_end_matches(['\r', '\n']).chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            _ => field.push(ch),
        }
    }
    fields.push(field);
    fields
}

fn fps_from_header_and_row(header: &str, row: &str) -> Option<f64> {
    let index = parse_csv_line(header)
        .iter()
        .position(|name| name.trim().eq_ignore_ascii_case(COLUMN))?;
    let raw = parse_csv_line(row).get(index)?.trim().parse::<f64>().ok()?;
    (raw.is_finite() && raw > 0.0).then_some(1000.0 / raw)
}

#[cfg(test)]
fn fps_from_csv(text: &str) -> Option<f64> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header = lines.next()?;
    let row = lines.last()?;
    fps_from_header_and_row(header, row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_frame_time_is_converted_to_frames_per_second() {
        let csv = "FrameNumber,MsBetweenPresents,ProcessID\n1,20.0,42\n2,16.0,42\n";
        assert_eq!(fps_from_csv(csv), Some(62.5));
    }

    #[test]
    fn the_column_is_found_by_name_not_by_position() {
        let csv = "\"Frame info, raw\",MsBetweenPresents,GPUUtilization\n\"a,b\",8,90\n";
        assert_eq!(fps_from_csv(csv), Some(125.0));
    }

    #[test]
    fn a_missing_or_unusable_frame_time_is_no_measurement() {
        assert_eq!(fps_from_csv("FrameNumber,GpuUtilization\n1,99\n"), None);
        assert_eq!(fps_from_csv("MsBetweenPresents\n0\n"), None);
        assert_eq!(fps_from_csv("MsBetweenPresents\nNaN\n"), None);
        assert_eq!(fps_from_csv("MsBetweenPresents\n\n"), None);
        assert_eq!(fps_from_csv(""), None);
    }

    #[test]
    fn a_stale_presentmon_log_is_not_used() {
        let dir = std::env::temp_dir().join(format!("meerkat-fps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, "MsBetweenPresents\n16\n").unwrap();
        assert_eq!(last_fps(&dir), (Some(62.5), FpsSource::PresentMon));

        let old = SystemTime::now() - MAX_AGE - Duration::from_secs(1);
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_modified(old).unwrap();
        assert_eq!(last_fps(&dir), (None, FpsSource::None));
        std::fs::remove_dir_all(&dir).ok();
    }
}
