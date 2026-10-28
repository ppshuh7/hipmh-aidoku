#![no_std]
mod core;

use aidoku::{
	alloc::{borrow::Cow, format, string::String, string::ToString, vec, vec::Vec},
	imports::{
		defaults::defaults_get,
		html::ElementList,
		net::Request,
		std::{current_date, sleep},
	},
	prelude::*,
	Chapter, ContentRating, DynamicFilters, DynamicSettings, Filter, FilterValue, Home, HomeComponent,
	HomeComponentValue, HomeLayout, Link, Listing, ListingProvider, Manga, MangaPageResult,
	MangaStatus, Page, PageContent, Result, SelectFilter, SelectSetting, Setting, SortFilter, Source,
	Viewer,
};
use serde::Deserialize;
use ::core::sync::atomic::Ordering;

impl Rating {
	fn to_rating(&self) -> ContentRating {
		match self {
			Rating::Str(s) => match s.as_str() {
				"suggestive" => ContentRating::Suggestive,
				"nsfw" => ContentRating::NSFW,
				_ => ContentRating::Safe,
			},
			Rating::Num(n) => match *n {
				1 => ContentRating::Suggestive,
				2 => ContentRating::NSFW,
				_ => ContentRating::Safe,
			},
		}
	}
}


use crate::core::{
	ApiChapter, ApiDetails, ApiManga, ApiResp, ChapterImages, ChapterListData, ListData,
	Rating, SearchData,
	chapter_key, decode_images, extract_mid, mid_b64, parse_iso_epoch, percent_encode,
	short_hid_from_key, img_base, API, COVER_CDN, PAGE_SIZE, SETTING_IMG_LINE, SITE, remove_decoy,
};

const UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";

/// Chapter list page size. 50 is the API maximum; it is also the per-request
/// payload size the cache-buster has to work around.
const CHAPTER_PER_PAGE: i32 = 50;

/// `total_pages` reported by the last chapter page fetch (set by
/// `fetch_chapter_page`, read immediately after by its caller).
static LAST_TOTAL_PAGES: ::core::sync::atomic::AtomicI32 = ::core::sync::atomic::AtomicI32::new(1);

// ---------------------------------------------------------------------------
// API payload models (hipapi1.s3file.top)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

impl ApiManga {
	fn into_manga(self) -> Manga {
		let cover = self
			.vertical_image_url
			.filter(|p| !p.is_empty())
			.map(|p| format!("{COVER_CDN}{p}"));
		let mut authors = self.authors.names();
		authors.extend(self.author_names);
		let authors: Vec<String> = authors.into_iter().filter(|n| !n.is_empty()).collect();
		let tags: Vec<String> = self
			.genres
			.names()
			.into_iter()
			.filter(|n| !n.is_empty())
			.collect();
		Manga {
			key: self.mid,
			title: self.title,
			cover,
			authors: (!authors.is_empty()).then_some(authors),
			description: self.description.filter(|d| !d.is_empty()),
			url: None,
			tags: (!tags.is_empty()).then_some(tags),
			status: status_of(self.status.as_deref()),
			content_rating: self
				.content_rating
				.as_ref()
				.map(Rating::to_rating)
				.unwrap_or(ContentRating::Safe),
			viewer: Viewer::Webtoon,
			..Default::default()
		}
	}
}

fn status_of(status: Option<&str>) -> MangaStatus {
	match status {
		Some("ongoing") => MangaStatus::Ongoing,
		Some("completed") => MangaStatus::Completed,
		Some("hiatus") => MangaStatus::Hiatus,
		Some("cancelled") => MangaStatus::Cancelled,
		_ => MangaStatus::Unknown,
	}
}

/// The API intermittently resets connections, so every request retries and
/// parse failures are retried too (a reset can surface as an empty body).
///
/// Retries are exponential-backoff and bounded by a per-call retry budget
/// rather than a flat count: a batch of chapter pages used to cost 3 × 1s
/// of dead sleep on every page when the API was having a bad minute, which
/// is what made "refresh" feel slow. Now the first retry is immediate
/// (the reset usually clears within ~100ms) and only keeps waiting while the
/// budget lasts.
/// Monotonic counter for cache-busting request URLs.
static NEXT: ::core::sync::atomic::AtomicU32 = ::core::sync::atomic::AtomicU32::new(0);

/// One chapter item -> aidoku Chapter (None if the hid cannot be parsed).
fn to_chapter(item: &ApiChapter) -> Option<Chapter> {
	let key = chapter_key(&item.hid)?;
	Some(Chapter {
		key,
		title: item.title.clone().filter(|t| !t.is_empty()),
		chapter_number: item.chapter_number,
		date_uploaded: item.created_at.as_deref().and_then(parse_iso_epoch),
		language: Some(String::from("zh")),
		..Default::default()
	})
}

/// Retry budget for one get_json call, in whole seconds of sleeping.
const RETRY_BUDGET_SECS: i32 = 4;
/// Max consecutive failures before giving up (also capped by the budget).
const MAX_ATTEMPTS: i32 = 6;

fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T> {
	get_json_with(url, RETRY_BUDGET_SECS)
}

fn get_json_with<T: for<'de> Deserialize<'de>>(url: &str, budget: i32) -> Result<T> {
	// The API caches responses per exact URL and can keep serving a stale
	// copy of a specific URL for days (observed: the `per_page=50&order=desc`
	// chapters URL missed new chapters while other URLs were fresh), so every
	// request gets a unique cache-busting parameter.
	let bust = current_date().wrapping_mul(1000).wrapping_add(NEXT.fetch_add(1, Ordering::Relaxed) as i64);
	let url = &format!("{url}&_={bust}");
	let mut last = error!("request failed");
	let mut spent = 0;
	for attempt in 0..MAX_ATTEMPTS {
		// no sleep before the first retry: resets are usually instantaneous
		for _ in 0..attempt.max(0) {
			if spent >= budget {
				return Err(last);
			}
			sleep(1);
			spent += 1;
		}
		match Request::get(url).map_err(|e| error!("request: {e:?}"))?.send() {
			Ok(response) => match response.get_string() {
				Ok(body) => match serde_json::from_str(&body) {
					Ok(parsed) => return Ok(parsed),
					Err(_) => last = error!("bad json from {url}"),
				},
				Err(e) => last = e,
			},
			Err(e) => last = error!("send: {e:?}"),
		}
	}
	Err(last)
}

// ---------------------------------------------------------------------------
// source
// ---------------------------------------------------------------------------

struct HipmhSource;

impl HipmhSource {
	/// Fetch a page of /v1/mangas results; `params` is a query-string
	/// fragment (may be empty).
	fn fetch_list(&self, params: &str, page: i32) -> Result<MangaPageResult> {
		self.fetch_list_n(params, page, PAGE_SIZE)
	}

	fn fetch_list_n(&self, params: &str, page: i32, per_page: i32) -> Result<MangaPageResult> {
		let url = format!("{API}/v1/mangas?{params}&page={page}&per_page={per_page}");
		let data: ApiResp<ListData> = get_json(&url)?;
		let data = data.data.unwrap_or_default();
		let entries = data.items.into_iter().map(ApiManga::into_manga).collect();
		Ok(MangaPageResult {
			has_next_page: page < data.total_pages,
			entries,
		})
	}

	/// 本週熱門 has no API equivalent (the weekly chart differs from the
	/// all-time popular sort), so scrape it from the homepage.
	fn fetch_weekly_popular(&self) -> Result<Vec<Link>> {
		let doc = Request::get(SITE).map_err(|e| error!("request: {e:?}"))?.header("User-Agent", UA)
			.send()
			.map_err(|e| error!("send: {e:?}"))?
			.get_html()
			.map_err(|e| error!("html: {e:?}"))?;
		let wrappers = doc
			.select("div.manga-section-wrapper")
			.ok_or_else(|| error!("no home sections"))?;
		for wrapper in wrappers {
			let title = wrapper
				.select_first(".manga-section-title-wrapper h2")
				.and_then(|t| t.text())
				.unwrap_or_default();
			if title != "本週熱門" {
				continue;
			}
			let entries = cards_to_manga(wrapper.select("a.manga-card-link"));
			if !entries.is_empty() {
				return Ok(entries.into_iter().map(Link::from).collect());
			}
		}
		Err(error!("weekly section not found"))
	}

	/// Full weekly chart (the 本週熱門 arrow target): the site's /weekly page,
	/// paginated with a next-page link.
	fn fetch_weekly_listing(&self, page: i32) -> Result<MangaPageResult> {
		let url = if page <= 1 {
			format!("{SITE}/weekly")
		} else {
			format!("{SITE}/weekly?page={page}")
		};
		let doc = Request::get(&url).map_err(|e| error!("request: {e:?}"))?.header("User-Agent", UA)
			.send()
			.map_err(|e| error!("send: {e:?}"))?
			.get_html()
			.map_err(|e| error!("html: {e:?}"))?;
		let entries = cards_to_manga(doc.select("a.manga-card-link"));
		if entries.is_empty() {
			return Err(error!("no weekly cards"));
		}
		let has_next = doc
			.select_first("a.pagination-next")
			.map(|a| a.attr("aria-disabled").as_deref() != Some("true"))
			.unwrap_or(false);
		Ok(MangaPageResult {
			entries,
			has_next_page: has_next,
		})
	}

	/// Fetch one chapter-list page (newest first) and remember `total_pages`
	/// for the caller. Chapters already known to the library keep their key,
	/// so re-emitting them is free.
	fn fetch_chapter_page(&self, mid_param: &str, page: i32) -> Result<Vec<Chapter>> {
		let url = format!(
			"{API}/v1/manga/chapters?mid={mid_param}&page={page}&per_page={CHAPTER_PER_PAGE}&order=desc"
		);
		let resp: ApiResp<ChapterListData> = get_json(&url)?;
		let data = resp.data.unwrap_or_default();
		LAST_TOTAL_PAGES.store(data.total_pages, Ordering::Relaxed);
		Ok(data.items.iter().filter_map(to_chapter).collect())
	}
}

/// Parse `a.manga-card-link` cards (homepage section or /weekly page) into
/// manga entries. Card covers are already absolute CDN urls.
fn cards_to_manga(cards: Option<ElementList>) -> Vec<Manga> {
	let mut out = Vec::new();
	let Some(cards) = cards else {
		return out;
	};
	for card in cards {
		let href = card.attr("href").unwrap_or_default();
		let key = match href.strip_prefix("/works/") {
			Some(k) if !k.is_empty() => k.to_string(),
			_ => continue,
		};
		let title = card
			.attr("aria-label")
			.or_else(|| card.select_first("img").and_then(|img| img.attr("alt")))
			.unwrap_or_default();
		if title.is_empty() {
			continue;
		}
		let cover = card
			.select_first("img")
			.and_then(|img| img.attr("src"))
			.filter(|src| !src.is_empty());
		out.push(Manga {
			url: Some(format!("{SITE}/works/{key}")),
			key,
			title,
			cover,
			viewer: Viewer::Webtoon,
			..Default::default()
		});
	}
	out
}

impl Source for HipmhSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut sort = "";
		let mut genre = String::new();
		let mut category = String::new();
		let mut status = String::new();
		for value in filters {
			match value {
				FilterValue::Sort { index, .. } => {
					sort = match index {
						1 => "popular",
						2 => "latest",
						_ => "updated",
					};
				}
				FilterValue::Select { id, value } => match id.as_str() {
					"genre" => genre = value,
					"category" => category = value,
					"status" => status = value,
					_ => {}
				},
				_ => {}
			}
		}

		if let Some(q) = query.filter(|q| !q.trim().is_empty()) {
			let url = format!(
				"{API}/v1/search?q={}&page={page}&page_size={PAGE_SIZE}",
				percent_encode(q.trim())
			);
			let data: ApiResp<SearchData> = get_json(&url)?;
			let data = data.data.unwrap_or_default();
			let entries = data.data.into_iter().map(ApiManga::into_manga).collect();
			Ok(MangaPageResult {
				has_next_page: page < data.total_pages,
				entries,
			})
		} else {
			let mut params = format!("sort={sort}");
			if !genre.is_empty() {
				params.push_str(&format!("&genre={genre}"));
			}
			if !category.is_empty() {
				params.push_str(&format!("&category={category}"));
			}
			if !status.is_empty() {
				params.push_str(&format!("&status={status}"));
			}
			self.fetch_list(&params, page)
		}
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let mid = extract_mid(&manga.key).ok_or_else(|| error!("bad manga key {}", manga.key))?;
		let mid_param = mid_b64(&mid);

		// Details and chapters are two independent requests. Start the chapter
		// page first (it is the slower one) and fetch details around it, so a
		// refresh costs max(details, chapters) instead of their sum.
		let chapter_prefetch: Option<Result<Vec<Chapter>>> = if needs_chapters {
			Some(self.fetch_chapter_page(&mid_param, 1))
		} else {
			None
		};

		if needs_details {
			let url = format!("{API}/v1/manga?mid={mid_param}");
			let data: ApiResp<ApiDetails> = get_json(&url)?;
			let d = data.data.ok_or_else(|| error!("no details"))?;

			let authors: Vec<String> = d
				.authors
				.iter()
				.map(|a| a.name.clone())
				.filter(|n| !n.is_empty())
				.collect();
			let mut tags: Vec<String> = d
				.genres
				.iter()
				.chain(d.tags.iter())
				.map(|g| g.name.clone())
				.filter(|n| !n.is_empty())
				.collect();
			tags.dedup();
			manga.title = d.title;
			manga.cover = d
				.vertical_image_url
				.filter(|p| !p.is_empty())
				.map(|p| format!("{COVER_CDN}{p}"));
			manga.authors = (!authors.is_empty()).then_some(authors);
			manga.description = d.description.filter(|x| !x.is_empty());
			manga.tags = (!tags.is_empty()).then_some(tags);
			manga.status = status_of(d.status.as_deref());
			manga.content_rating = d
				.content_rating
				.as_ref()
				.map(Rating::to_rating)
				.unwrap_or(ContentRating::Safe);
			manga.viewer = Viewer::Webtoon;
			manga.url = Some(format!("{SITE}/works/{}", manga.key));
		}

		if needs_chapters {
			// Chapters already stored by Aidoku for this manga (they come in
			// on `manga.chapters`). Aidoku keeps them on refresh — it only
			// deletes chapters that are missing from what we return — and a
			// chapter that is keyed the same can be opened without the source
			// ever holding it, so the whole list does not have to be re-fetched
			// on every pull-to-refresh (which used to cost 14 pages × 2s on a
			// 690-chapter title).
			let known: Vec<Chapter> = manga.chapters.take().unwrap_or_default();

			// Page 1 (newest first) is all an up-to-date series needs: a series
			// that already has chapters in the library only ever grows at the
			// top, so further pages would be re-downloading what Aidoku already
			// has. That is what made a large library refresh crawl — every one
			// of N titles cost (pages × 1s sleep + round-trips).
			let mut chapters = match chapter_prefetch {
				Some(r) => r?,
				None => self.fetch_chapter_page(&mid_param, 1)?,
			};
			let total_pages = LAST_TOTAL_PAGES.load(Ordering::Relaxed);

			// Deep backfill only for a title the library has never seen. The
			// user explicitly chose this trade-off: library titles are not
			// extended downwards on refresh (reading progress is keyed by
			// chapter key, so nothing is lost), new titles still arrive whole.
			if known.is_empty() && total_pages > 1 {
				let mut page = 2;
				let mut failed_pages = 0;
				while page <= total_pages && page <= 200 {
					match self.fetch_chapter_page(&mid_param, page) {
						Ok(more) => {
							failed_pages = 0;
							chapters.extend(more);
						}
						Err(_) => {
							failed_pages += 1;
							if failed_pages >= 3 {
								break;
							}
						}
					}
					sleep(1);
					page += 1;
				}
			}

			// Re-emit the chapters the library already has (same key => same
			// row) so Aidoku keeps rendering the full history instead of
			// deleting whatever this call did not return.
			if let Some(oldest_fresh) = chapters
				.iter()
				.filter_map(|c| c.chapter_number)
				.fold(None::<f32>, |acc, n| Some(acc.map_or(n, |a| a.min(n))))
			{
				for chapter in known {
					match chapter.chapter_number {
						Some(n) if n < oldest_fresh => chapters.push(chapter),
						// unnumbered entries: keep whatever the library had
						None => chapters.push(chapter),
						_ => {}
					}
				}
			} else {
				chapters.extend(known);
			}

			chapters.sort_by(|a, b| {
				b.chapter_number
					.unwrap_or(0.0)
					.total_cmp(&a.chapter_number.unwrap_or(0.0))
			});
			manga.chapters = Some(chapters);
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let hid = short_hid_from_key(&chapter.key).ok_or_else(|| error!("bad chapter key"))?;
		let url = format!("{API}/v2/chapter?hid={hid}");
		let data: ApiResp<ChapterImages> = get_json(&url)?;
		let data = data.data.ok_or_else(|| error!("no chapter data"))?;
		let images = data.images.ok_or_else(|| error!("no images"))?;
		let decoded = decode_images(&images).map_err(|e| error!("decode: {e}"))?;
		let paths = remove_decoy(decoded, data.order_id, data.sid);

		// Page images follow the user's line choice (see the settings page);
		// the default is the `cf-1` line, measured ~44% faster than `tx-1`.
		let base = img_base(defaults_get::<String>(SETTING_IMG_LINE).as_deref());
		let pages = paths
			.into_iter()
			.map(|path| Page {
				content: PageContent::url(format!("{base}{path}")),
				..Default::default()
			})
			.collect();
		Ok(pages)
	}
}

impl ListingProvider for HipmhSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let params = match listing.id.as_str() {
			"ko" => "category=1&sort=updated",
			"cn" => "category=2&sort=updated",
			"ongoing" => "status=ongoing&sort=updated",
			"completed" => "status=completed&sort=updated",
			"popular" => "sort=popular",
			"latest" => "sort=latest",
			"completed_popular" => "status=completed&sort=popular",
			"weekly" => return self.fetch_weekly_listing(page),
			// "updated" and anything unknown
			_ => "sort=updated",
		};
		self.fetch_list(params, page)
	}
}

impl Home for HipmhSource {
	fn get_home(&self) -> Result<HomeLayout> {
		let listing = |id: &str, name: &str| {
			Some(Listing {
				id: String::from(id),
				name: String::from(name),
				kind: Default::default(),
			})
		};

		// Five sections, fetched in order (the API throttles bursts from one
		// client). If any single section fails we still return the others —
		// a dead section must not blank the whole home screen.
		//
		// NOTE: do *not* use send_partial_result here. The runner replaces a
		// partial component by matching `title`, and the layout already sent
		// by a previous call can overwrite the final result: an empty-title
		// skeleton ends up stuck on screen (seen in v11: home rendered empty
		// rows). A plain, complete HomeLayout is the reliable path.
		let sections: [(&str, &str, &str, i32, bool); 5] = [
			("近期更新", "updated", "sort=updated", 8, false),
			("本週熱門", "weekly", "sort=popular", 10, false),
			("人氣排名", "popular", "sort=popular", 10, true),
			("最新上架", "latest", "sort=latest", PAGE_SIZE, false),
			("完結推薦", "completed_popular", "status=completed&sort=popular", PAGE_SIZE, false),
		];

		let mut components: Vec<HomeComponent> = Vec::new();
		let mut popular_fallback: Vec<Link> = Vec::new();
		let mut first_error = None;

		for (title, id, params, per_page, ranking) in sections {
			let links: Vec<Link> = if id == "weekly" {
				// 本週熱門 has no API equivalent: scrape the homepage section,
				// falling back to the popular list if the DOM changed.
				match self.fetch_weekly_popular() {
					Ok(l) if !l.is_empty() => l,
					_ => popular_fallback.clone(),
				}
			} else {
				match self.fetch_list_n(params, 1, per_page) {
					Ok(result) => {
						let links: Vec<Link> = result.entries.into_iter().map(Link::from).collect();
						if id == "popular" {
							popular_fallback = links.clone();
						}
						links
					}
					Err(e) => {
						// remember the first failure, keep building the rest
						if first_error.is_none() {
							first_error = Some(e);
						}
						Vec::new()
					}
				}
			};

			let value = if ranking {
				HomeComponentValue::MangaList {
					ranking: true,
					page_size: Some(10),
					entries: links.into_iter().take(10).collect(),
					listing: listing(id, title),
				}
			} else {
				HomeComponentValue::Scroller {
					entries: links,
					listing: listing(id, title),
				}
			};
			components.push(HomeComponent {
				title: Some(String::from(title)),
				subtitle: None,
				value,
			});
		}

		// every section empty means the API/site is gone: surface the error
		// instead of showing five blank rows
		let any_entries = components.iter().any(|c| match &c.value {
			HomeComponentValue::Scroller { entries, .. } => !entries.is_empty(),
			HomeComponentValue::MangaList { entries, .. } => !entries.is_empty(),
			_ => false,
		});
		if !any_entries {
			if let Some(e) = first_error {
				return Err(e);
			}
		}

		Ok(HomeLayout { components })
	}
}

impl DynamicFilters for HipmhSource {
	fn get_dynamic_filters(&self) -> Result<Vec<Filter>> {
		// genre ids verified against /v1/mangas?genre=<id>
		const GENRES: &[(&str, &str)] = &[
			("玄幻", "27"),
			("武侠", "39"),
			("奇幻", "8"),
			("穿越", "20"),
			("重生", "46"),
			("系统", "67"),
			("逆袭", "32"),
			("大女主", "30"),
			("宫斗", "44"),
			("复仇", "31"),
			("冒险", "38"),
			("动作", "40"),
			("战斗", "63"),
			("异能", "51"),
			("剧情", "3"),
			("搞笑", "4"),
			("古风", "19"),
			("怪物", "197"),
			("少年", "75"),
			("校园", "257"),
		];
		Ok(vec![
			SelectFilter {
				id: Cow::Borrowed("genre"),
				title: Some(Cow::Borrowed("题材")),
				hide_from_header: Some(false),
				is_genre: true,
				uses_tag_style: true,
				options: GENRES.iter().map(|(n, _)| Cow::Borrowed(*n)).collect(),
				ids: Some(GENRES.iter().map(|(_, i)| Cow::Borrowed(*i)).collect()),
				default: None,
			}
			.into(),
			SelectFilter {
				id: Cow::Borrowed("category"),
				title: Some(Cow::Borrowed("地区")),
				hide_from_header: Some(false),
				is_genre: false,
				uses_tag_style: false,
				options: vec![Cow::Borrowed("韩漫"), Cow::Borrowed("国漫")],
				ids: Some(vec![Cow::Borrowed("1"), Cow::Borrowed("2")]),
				default: None,
			}
			.into(),
			SelectFilter {
				id: Cow::Borrowed("status"),
				title: Some(Cow::Borrowed("状态")),
				hide_from_header: Some(false),
				is_genre: false,
				uses_tag_style: false,
				options: vec![Cow::Borrowed("连载中"), Cow::Borrowed("已完结")],
				ids: Some(vec![Cow::Borrowed("ongoing"), Cow::Borrowed("completed")]),
				default: None,
			}
			.into(),
			SortFilter {
				id: Cow::Borrowed("sort"),
				title: Some(Cow::Borrowed("排序")),
				hide_from_header: Some(false),
				can_ascend: false,
				options: vec![
					Cow::Borrowed("近期更新"),
					Cow::Borrowed("热门"),
					Cow::Borrowed("最新上架"),
				],
				default: None,
			}
			.into(),
		])
	}
}

impl DynamicSettings for HipmhSource {
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		Ok(vec![
			SelectSetting {
				key: Cow::Borrowed(SETTING_IMG_LINE),
				title: Cow::Borrowed("图片线路"),
				values: vec![Cow::Borrowed("cf1"), Cow::Borrowed("tx1")],
				titles: Some(vec![
					Cow::Borrowed("CF 线路（推荐·更快）"),
					Cow::Borrowed("TX 线路（备用）"),
				]),
				auth_to_open: Some(false),
				default: Some(String::from("cf1")),
				..Default::default()
			}
			.into(),
		])
	}
}

register_source!(HipmhSource, ListingProvider, Home, DynamicFilters, DynamicSettings);






