//! Bing Webmaster Tools' AI Performance overview export
//! (`Date, Citations, Cited Pages`), read with the same rules as aieo.ee's
//! importer (`packages/core/src/bing-ai.ts`): UTF-8 (BOM optional) or UTF-16
//! with a BOM; that exact header; dates `M/D/YYYY h:mm:ss AM/PM`; whole
//! numbers (thousands separators allowed); cited pages ≤ citations; one row
//! per day, no gaps, nothing after today. A file outside these rules is
//! refused, never guessed at.

use anyhow::{bail, Result};
use time::{Date, Month};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverviewDay {
    pub day: Date,
    pub citations: u64,
    pub cited_pages: u64,
}

pub fn decode(bytes: &[u8]) -> Result<String> {
    let utf16 = |b: &[u8], le: bool| -> Result<String> {
        if !b.len().is_multiple_of(2) {
            bail!("the file is not valid UTF-16");
        }
        let units: Vec<u16> = b
            .chunks(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| anyhow::anyhow!("the file is not valid UTF-16"))
    };
    match bytes {
        [0xff, 0xfe, rest @ ..] => utf16(rest, true),
        [0xfe, 0xff, rest @ ..] => utf16(rest, false),
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8(rest.to_vec())
            .map_err(|_| anyhow::anyhow!("the file is not valid UTF-8")),
        _ => String::from_utf8(bytes.to_vec()).map_err(|_| {
            anyhow::anyhow!("the file is not valid UTF-8, or UTF-16 with a byte-order mark")
        }),
    }
}

/// RFC 4180 records (quoted fields, doubled quotes, CRLF or LF).
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                row.push(std::mem::take(&mut field));
                if !(row.len() == 1 && row[0].is_empty()) {
                    rows.push(std::mem::take(&mut row));
                }
                row.clear();
            }
            _ => field.push(c),
        }
    }
    row.push(field);
    if !(row.len() == 1 && row[0].is_empty()) {
        rows.push(row);
    }
    rows
}

/// `M/D/YYYY h:mm:ss AM/PM` → date (the time is not used).
pub fn parse_date(raw: &str) -> Option<Date> {
    let (date, rest) = raw.split_once(' ')?;
    let (time, ampm) = rest.split_once(' ')?;
    if !matches!(ampm, "AM" | "PM") {
        return None;
    }
    let mut d = date.split('/');
    let (mo, day, y): (u8, u8, i32) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    if d.next().is_some() || !(2000..=2100).contains(&y) {
        return None;
    }
    let mut t = time.split(':');
    let (h, mi, s): (u8, u8, u8) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    if !(1..=12).contains(&h) || mi > 59 || s > 59 || t.next().is_some() {
        return None;
    }
    Date::from_calendar_date(y, Month::try_from(mo).ok()?, day).ok()
}

/// `1234` or `1,234`.
pub fn parse_count(raw: &str) -> Option<u64> {
    let ok = raw.chars().all(|c| c.is_ascii_digit())
        || raw.split(',').enumerate().all(|(i, g)| {
            !g.is_empty()
                && g.chars().all(|c| c.is_ascii_digit())
                && if i == 0 { g.len() <= 3 } else { g.len() == 3 }
        });
    if !ok || raw.is_empty() {
        return None;
    }
    raw.replace(',', "")
        .parse()
        .ok()
        .filter(|n| *n <= 2_147_483_647)
}

pub fn parse_overview(bytes: &[u8], today: Date) -> Result<Vec<OverviewDay>> {
    let rows = parse_csv(&decode(bytes)?);
    let header: Vec<String> = rows
        .first()
        .map(|h| {
            h.iter()
                .map(|c| {
                    c.trim()
                        .to_lowercase()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect()
        })
        .unwrap_or_default();
    if header != ["date", "citations", "cited pages"] {
        bail!(
            "not an AI Performance overview export (expected the columns Date, Citations, Cited Pages; got {})",
            header.join(", ")
        );
    }
    let mut days = Vec::new();
    for (i, r) in rows.iter().enumerate().skip(1) {
        let line = i + 1;
        if r.len() != 3 {
            bail!("line {line} has {} columns, expected 3", r.len());
        }
        let Some(day) = parse_date(&r[0]) else {
            bail!(
                "line {line}: unsupported date {:?} — only M/D/YYYY h:mm:ss AM/PM is accepted",
                r[0]
            );
        };
        let (Some(citations), Some(cited_pages)) = (parse_count(&r[1]), parse_count(&r[2])) else {
            bail!("line {line}: Citations and Cited Pages must be non-negative whole numbers");
        };
        if cited_pages > citations {
            bail!("line {line}: Cited Pages is larger than Citations");
        }
        days.push(OverviewDay {
            day,
            citations,
            cited_pages,
        });
    }
    if days.is_empty() {
        bail!("the file has a header but no rows");
    }
    days.sort_by_key(|d| d.day);
    for w in days.windows(2) {
        if w[0].day == w[1].day {
            bail!("{} appears twice", w[0].day);
        }
        if w[0].day.next_day() != Some(w[1].day) {
            bail!(
                "the dates are not continuous: nothing between {} and {}",
                w[0].day,
                w[1].day
            );
        }
    }
    let last = days.last().expect("rows").day;
    if last > today {
        bail!("the file ends on {last}, after today ({today}, UTC)");
    }
    Ok(days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn reads_the_overview_export() {
        let csv = "\u{feff}Date,Citations,Cited Pages\r\n10/1/2026 12:00:00 AM,\"1,204\",31\r\n10/2/2026 12:00:00 AM,0,0\r\n";
        let days = parse_overview(csv.as_bytes(), date!(2026 - 10 - 22)).unwrap();
        assert_eq!(
            days[0],
            OverviewDay {
                day: date!(2026 - 10 - 01),
                citations: 1204,
                cited_pages: 31
            }
        );
        assert_eq!(days.len(), 2);
        let utf16: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain(
                "Date,Citations,Cited Pages\n10/1/2026 1:00:00 PM,2,1\n"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes),
            )
            .collect();
        assert_eq!(
            parse_overview(&utf16, date!(2026 - 10 - 22)).unwrap().len(),
            1
        );
    }

    #[test]
    fn refuses_what_it_cannot_vouch_for() {
        let t = date!(2026 - 10 - 22);
        let e = |s: &str| parse_overview(s.as_bytes(), t).unwrap_err().to_string();
        assert!(e("Grounding Query,Intent,Topic,Citations,Citation Share\n")
            .contains("not an AI Performance overview"));
        assert!(e("Date,Citations,Cited Pages\n2026-10-01,1,1\n").contains("unsupported date"));
        assert!(e("Date,Citations,Cited Pages\n10/1/2026 12:00:00 AM,1,2\n").contains("larger"));
        assert!(e(
            "Date,Citations,Cited Pages\n10/1/2026 12:00:00 AM,1,1\n10/3/2026 12:00:00 AM,1,1\n"
        )
        .contains("not continuous"));
        assert!(
            e("Date,Citations,Cited Pages\n10/30/2026 12:00:00 AM,1,1\n").contains("after today")
        );
        assert!(
            e("Date,Citations,Cited Pages\n10/1/2026 12:00:00 AM,1.5,1\n")
                .contains("whole numbers")
        );
        assert_eq!(parse_count("12,345"), Some(12345));
        assert_eq!(parse_count("1,23"), None);
    }
}
