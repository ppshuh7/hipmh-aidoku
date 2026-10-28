// Pure, host-testable core logic: base64url, the site's ID scheme, and the
// chapter image deobfuscation. No aidoku dependency, so it can be shared by
// the wasm source and the host-side differential test programs.
extern crate alloc;

use alloc::{format, string::String, string::ToString, vec::Vec};
use serde::Deserialize;

pub const API: &str = "https://hipapi1.s3file.top";
pub const SITE: &str = "https://m.hipmh.com";
pub const COVER_CDN: &str = "https://cover.s3imgs.top";

/// Chapter image hosts, ordered by measured speed from this network.
///
/// The reader page ships `data-chapter-img-base-line1/line2` and its JS uses
/// `line1` unless the user switched lines (`localStorage["chapterApiLine"]`).
/// Measured over 3 rounds × 10 sequential images + 6 concurrent prefetches:
///   cf-1 line: 1.58s/img sequential, 1.75s per 6-image batch
///   tx-1 line: 2.83s/img sequential, 3.19s per batch (and it resets
///              connections under concurrency)
/// Mixing the two is *worse* than cf-1 alone (2.53s/img) because every tx-1
/// image drags the batch down, so the first entry is used for everything and
/// the rest is only a documented fallback.
pub const IMG_HOSTS: [&str; 2] = [
	"https://hip-cf-1.s3imgs.top",
	"https://hip-tx-1.s3imgs.top",
];

/// The single host used for page images (see IMG_HOSTS for why).
pub const IMG_CDN: &str = IMG_HOSTS[0];
/// Thumbnail (`-s1`) line, documented for completeness; the reader does not
/// use it (the full-size line serves thumbnails fine).
#[allow(dead_code)]
pub const IMG_CDN_SMALL: &str = "https://hip-cf-s1.s3imgs.top";

pub const PAGE_SIZE: i32 = 20;

// ---------------------------------------------------------------------------
// base64url (RFC 4648 §5, unpadded output; accepts -_ and +/ input)
// ---------------------------------------------------------------------------

const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn b64url_encode(s: &str) -> String {
	let bytes = s.as_bytes();
	let mut out = Vec::with_capacity((bytes.len() + 2) / 3 * 4);
	for chunk in bytes.chunks(3) {
		let b0 = chunk[0] as u32;
		let b1 = *chunk.get(1).unwrap_or(&0) as u32;
		let b2 = *chunk.get(2).unwrap_or(&0) as u32;
		let n = (b0 << 16) | (b1 << 8) | b2;
		out.push(B64URL[(n >> 18) as usize & 63]);
		out.push(B64URL[(n >> 12) as usize & 63]);
		out.push(if chunk.len() > 1 { B64URL[(n >> 6) as usize & 63] } else { b'=' });
		out.push(if chunk.len() > 2 { B64URL[n as usize & 63] } else { b'=' });
	}
	while out.last() == Some(&b'=') {
		out.pop();
	}
	String::from_utf8(out).unwrap_or_default()
}

fn b64_val(c: u8) -> Option<u32> {
	match c {
		b'A'..=b'Z' => Some((c - b'A') as u32),
		b'a'..=b'z' => Some((c - b'a' + 26) as u32),
		b'0'..=b'9' => Some((c - b'0' + 52) as u32),
		b'-' | b'+' => Some(62),
		b'_' | b'/' => Some(63),
		_ => None,
	}
}

pub fn b64url_decode(s: &str) -> Option<Vec<u8>> {
	let mut out = Vec::with_capacity(s.len() * 3 / 4 + 3);
	let mut acc: u32 = 0;
	let mut bits: u32 = 0;
	for &c in s.as_bytes() {
		if c == b'=' {
			break;
		}
		acc = (acc << 6) | b64_val(c)?;
		bits += 6;
		if bits >= 8 {
			bits -= 8;
			out.push((acc >> bits) as u8);
		}
	}
	Some(out)
}

#[derive(Deserialize)]
pub struct ApiResp<T> {
	#[serde(default)]
	pub data: Option<T>,
}

#[derive(Deserialize)]
#[serde(default)]
pub struct SearchData {
	pub data: Vec<ApiManga>,
	pub total_pages: i32,
}
impl Default for SearchData {
	fn default() -> Self {
		Self { data: Vec::new(), total_pages: 0 }
	}
}

#[derive(Deserialize)]
#[serde(default)]
pub struct ListData {
	pub items: Vec<ApiManga>,
	pub total_pages: i32,
}
impl Default for ListData {
	fn default() -> Self {
		Self { items: Vec::new(), total_pages: 0 }
	}
}

/// content_rating is "safe"/"suggestive"/"nsfw" in some payloads and 0/1/2 in
/// others.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Rating {
	Str(String),
	Num(i64),
}

#[derive(Deserialize)]
#[serde(default)]
pub struct ApiName {
	pub name: String,
}
impl Default for ApiName {
	fn default() -> Self {
		Self { name: String::new() }
	}
}

/// Search items carry `authors: [{name}]` / `genres: [{name}]` while browse
/// items carry `author_names: ["..."]` / `genres: ["..."]` — accept both.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum NameList {
	Objects(Vec<ApiName>),
	Strings(Vec<String>),
}

impl NameList {
	pub fn names(&self) -> Vec<String> {
		match self {
			NameList::Objects(v) => v.iter().map(|n| n.name.clone()).filter(|n| !n.is_empty()).collect(),
			NameList::Strings(v) => v.clone(),
		}
	}
}

#[derive(Deserialize)]
#[serde(default)]
pub struct ApiManga {
	/// search items call it "id", browse items call it "mid"
	#[serde(alias = "id")]
	pub mid: String,
	pub title: String,
	pub vertical_image_url: Option<String>,
	pub description: Option<String>,
	pub authors: NameList,
	pub author_names: Vec<String>,
	pub genres: NameList,
	pub status: Option<String>,
	pub content_rating: Option<Rating>,
}
impl Default for ApiManga {
	fn default() -> Self {
		Self {
			mid: String::new(),
			title: String::new(),
			vertical_image_url: None,
			description: None,
			authors: NameList::Strings(Vec::new()),
			author_names: Vec::new(),
			genres: NameList::Strings(Vec::new()),
			status: None,
			content_rating: None,
		}
	}
}

#[derive(Deserialize)]
#[serde(default)]
pub struct ApiDetails {
	pub title: String,
	pub description: Option<String>,
	pub authors: Vec<ApiName>,
	pub genres: Vec<ApiName>,
	pub tags: Vec<ApiName>,
	pub status: Option<String>,
	pub content_rating: Option<Rating>,
	pub vertical_image_url: Option<String>,
}
impl Default for ApiDetails {
	fn default() -> Self {
		Self {
			title: String::new(),
			description: None,
			authors: Vec::new(),
			genres: Vec::new(),
			tags: Vec::new(),
			status: None,
			content_rating: None,
			vertical_image_url: None,
		}
	}
}

#[derive(Deserialize)]
#[serde(default)]
pub struct ChapterListData {
	pub items: Vec<ApiChapter>,
	pub total_pages: i32,
}
impl Default for ChapterListData {
	fn default() -> Self {
		Self { items: Vec::new(), total_pages: 0 }
	}
}

#[derive(Deserialize)]
#[serde(default)]
pub struct ApiChapter {
	pub hid: String,
	pub chapter_number: Option<f32>,
	pub title: Option<String>,
	pub created_at: Option<String>,
}
impl Default for ApiChapter {
	fn default() -> Self {
		Self { hid: String::new(), chapter_number: None, title: None, created_at: None }
	}
}

#[derive(Deserialize)]
#[serde(default)]
pub struct ChapterImages {
	pub images: Option<String>,
	pub order_id: u64,
	pub sid: u64,
}
impl Default for ChapterImages {
	fn default() -> Self {
		Self { images: None, order_id: 0, sid: 0 }
	}
}


// ---------------------------------------------------------------------------
// ID scheme (all verified against the live site)
//
// mid:        b64url("m:<digits>")
// works id:   "<mid>-<slug>[-<more>]" — what search/list/home hand out
// full hid:   b64url("m:<mid>-c:<cid>") + "-" + b64url("<mid>:<num>.00")
// short hid:  b64url("c:<cid>") + "-" + <second segment kept verbatim>
//             (only the short hid is accepted by /v2/chapter)
// chapter key: "<cid>|<second segment>" (stored on Chapter.key)
// ---------------------------------------------------------------------------

/// Walk '-'-joined prefixes of a works id until one base64url-decodes to "m:<digits>".
pub fn extract_mid(full_id: &str) -> Option<String> {
	let parts: Vec<&str> = full_id.split('-').collect();
	for n in 1..=parts.len() {
		let prefix = parts[..n].join("-");
		let bytes = b64url_decode(&prefix)?;
		let text = String::from_utf8(bytes).ok()?;
		if let Some(rest) = text.strip_prefix("m:") {
			if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
				return Some(rest.to_string());
			}
		}
	}
	None
}

/// b64url of "m:<mid>" — the form /v1/manga and /v1/manga/chapters expect.
pub fn mid_b64(mid: &str) -> String {
	b64url_encode(&format!("m:{mid}"))
}

fn looks_like_part2(text: &str) -> bool {
	let mut seen_colon = false;
	let mut seen_dot = false;
	let mut any_digit = false;
	for c in text.chars() {
		match c {
			'0'..='9' => any_digit = true,
			':' if !seen_dot => seen_colon = true,
			'.' if seen_colon => seen_dot = true,
			_ => return false,
		}
	}
	any_digit && seen_colon && seen_dot
}

/// Find the hid separator: scan from the right for a '-' whose right side
/// base64url-decodes to "<digits>:<digits>.<digits>".
pub fn split_hid(full_hid: &str) -> Option<(&str, &str)> {
	for (i, c) in full_hid.char_indices().rev() {
		if c != '-' {
			continue;
		}
		let (p1, p2) = (&full_hid[..i], &full_hid[i + 1..]);
		if p1.is_empty() || p2.is_empty() {
			continue;
		}
		let bytes = b64url_decode(p2)?;
		let text = String::from_utf8(bytes).ok()?;
		if looks_like_part2(&text) {
			return Some((p1, p2));
		}
	}
	None
}

fn part1_cid(part1: &str) -> Option<String> {
	let text = String::from_utf8(b64url_decode(part1)?).ok()?;
	// "m:<mid>-c:<cid>" (full hid) or "c:<cid>" (short hid)
	if let Some(rest) = text.strip_prefix("m:") {
		let idx = rest.find("-c:")?;
		Some(rest[idx + 3..].to_string())
	} else if let Some(cid) = text.strip_prefix("c:") {
		Some(cid.to_string())
	} else {
		None
	}
}

/// Chapter key stored on Chapter.key: "<cid>|<part2>".
pub fn chapter_key(full_hid: &str) -> Option<String> {
	let (p1, p2) = split_hid(full_hid)?;
	Some(format!("{}|{}", part1_cid(p1)?, p2))
}

/// Rebuild the short hid the /v2/chapter endpoint accepts.
pub fn short_hid_from_key(key: &str) -> Option<String> {
	let (cid, part2) = key.split_once('|')?;
	Some(format!("{}-{}", b64url_encode(&format!("c:{cid}")), part2))
}

// ---------------------------------------------------------------------------
// Chapter image deobfuscation (byte-for-byte identical to the site's
// chapter-decoder.js; differentially verified against it)
// ---------------------------------------------------------------------------

const IMG_PREFIX: &[u8; 3] = b"qM9";
const IMG_SUFFIX: &[u8; 2] = b"Z7";
const SEP: &[u8; 2] = b"Vx";
const MARKER: &[u8; 3] = b"pL0";
/// Scrambled alphabet used by the payload, mapped position-wise onto STD
/// (recovered differentially from the site decoder; the three chars V/Y/Z
/// never occur in real payloads but complete the permutation).
const SCR: &[u8; 64] = b"_-9876543210abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Decode the obfuscated `images` field of /v2/chapter into relative image
/// paths (still including the decoy entry).
pub fn decode_images(images: &str) -> Result<Vec<String>, &'static str> {
	let b = images.as_bytes();
	let body = if b.len() > IMG_PREFIX.len() + IMG_SUFFIX.len()
		&& b.starts_with(IMG_PREFIX)
		&& b.ends_with(IMG_SUFFIX)
	{
		&b[IMG_PREFIX.len()..b.len() - IMG_SUFFIX.len()]
	} else {
		return Err("missing qM9/Z7 envelope");
	};

	let n = body.len().checked_sub(SEP.len() + MARKER.len()).ok_or("body too short")?;
	if n == 0 {
		return Err("empty payload");
	}
	let tag_len = n / 3;
	let n1 = (n - tag_len) / 2;
	let n2 = n - n1 - tag_len;

	let h1 = &body[..n1];
	let sep = &body[n1..n1 + SEP.len()];
	let h2 = &body[n1 + SEP.len()..n1 + SEP.len() + n2];
	let marker = &body[n1 + SEP.len() + n2..n1 + SEP.len() + n2 + MARKER.len()];
	let tag = &body[n1 + SEP.len() + n2 + MARKER.len()..];
	if sep != &SEP[..] {
		return Err("Vx separator missing");
	}
	if marker != &MARKER[..] {
		return Err("pL0 marker missing");
	}
	if tag.len() != tag_len {
		return Err("tag length mismatch");
	}

	// reassemble as tag + h1 + h2, then invert odd 7-char blocks
	let combined: Vec<u8> = tag.iter().chain(h1.iter()).chain(h2.iter()).cloned().collect();
	let mut reordered = Vec::with_capacity(combined.len());
	for (j, chunk) in combined.chunks(7).enumerate() {
		if j % 2 == 1 {
			reordered.extend(chunk.iter().rev());
		} else {
			reordered.extend(chunk.iter().cloned());
		}
	}

	// scrambled -> standard base64 alphabet, then decode
	let mut mapped = Vec::with_capacity(reordered.len());
	for &c in &reordered {
		let k = SCR.iter().position(|&s| s == c).ok_or("bad char in payload")?;
		mapped.push(STD[k]);
	}
	while mapped.len() % 4 != 0 {
		mapped.push(b'=');
	}

	let decoded = b64url_decode(str::from_utf8(&mapped).map_err(|_| "not ascii")?)
		.ok_or("base64 decode failed")?;
	serde_json::from_slice(&decoded).map_err(|_| "bad json")
}

/// Drop the decoy entry the site injects.
/// h = order_id ^ ((sid * 0x9E3779B1) ^ (len * 0x85EBCA6B)) mod len
pub fn remove_decoy(mut images: Vec<String>, order_id: u64, sid: u64) -> Vec<String> {
	let len = images.len() as u64;
	if len == 0 {
		return images;
	}
	let h = order_id ^ ((sid.wrapping_mul(0x9E37_79B1) ^ len.wrapping_mul(0x85EB_CA6B)) % len);
	if h < len {
		images.remove(h as usize);
	}
	images
}

/// Parse "2026-09-04T05:53:32.323887Z" into a unix timestamp (UTC assumed).
pub fn parse_iso_epoch(s: &str) -> Option<i64> {
	let b = s.as_bytes();
	if b.len() < 19 {
		return None;
	}
	let num = |from: usize, to: usize| -> Option<i64> { str::from_utf8(&b[from..to]).ok()?.parse::<i64>().ok() };
	if b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') || b[13] != b':' || b[16] != b':' {
		return None;
	}
	let y = num(0, 4)?;
	let mo = num(5, 7)?;
	let d = num(8, 10)?;
	let h = num(11, 13)?;
	let mi = num(14, 16)?;
	let sec = num(17, 19)?;
	if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
		return None;
	}
	// days from civil (Howard Hinnant's algorithm)
	let y = y - if mo <= 2 { 1 } else { 0 };
	let era = if y >= 0 { y } else { y - 399 } / 400;
	let yoe = y - era * 400;
	let mp = (mo + 9) % 12;
	let doy = (153 * mp + 2) / 5 + d - 1;
	let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
	let days = era * 146097 + doe - 719468;
	Some(days * 86400 + h * 3600 + mi * 60 + sec)
}

/// Percent-encode a query component (UTF-8).
pub fn percent_encode(s: &str) -> String {
	let mut out = Vec::with_capacity(s.len() * 3);
	for &c in s.as_bytes() {
		match c {
			b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(c),
			_ => {
				out.push(b'%');
				out.push(b"0123456789ABCDEF"[(c >> 4) as usize]);
				out.push(b"0123456789ABCDEF"[(c & 15) as usize]);
			}
		}
	}
	String::from_utf8(out).unwrap_or_default()
}
