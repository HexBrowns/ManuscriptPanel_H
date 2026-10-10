//! Markdown の原稿 → 行の一覧と、置いたテロップとの照合（本体と関係なく、cargo test の中心）。
//!
//! 行ごとに読む（汎用の Markdown パーサーは使わない）。1 行 = 1 テロップが既定で、行番号をそのまま持ちたいことと、
//! AviUtl2 の制御文字（`<$強調>` `<#色>`）を HTML として扱わずに残したいことによる。
//!
//! - 一覧から外す: YAML の前置き、HTML コメント `<!-- -->`、Obsidian のコメント `%% %%`、コードブロック、水平線、表の区切り行、空行
//! - 置く本文から外す記法: 太字・斜体・取り消し線・ハイライト・コードの記号、`[文字](URL)` は文字だけ、`[[リンク|表示名]]` は表示名、
//!   画像と埋め込み、脚注、行頭のリスト・番号・引用・チェックボックス、行末のブロック ID（`^abc`）
//! - カット番号: 行の中の「カット番号のパターン」に当たる部分を取り出して、本文からは外す。実物の原稿は行頭の `【N】`
//!   （仕様の「未確認」の 1）。当たらない行は既定で一覧に出さない
//! - 画面メモ: 行頭が `▸` の行は、直前のカットのメモにする（テロップにしない）

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

/// 1 テロップの単位
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Unit {
    /// 1 行 = 1 テロップ（既定）
    Line,
    /// 空行で区切った段落 = 1 テロップ（段落の中の改行は保つ）
    Paragraph,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub unit: Unit,
    /// カット番号のパターン（正規表現）。空なら使わない。1 番目の括弧があればそれを、無ければ当たった全体を番号にする
    pub cut_pattern: String,
    /// 画面メモの行頭の文字。空なら使わない
    pub memo_prefix: String,
    /// カット番号に当たらない行も一覧に出す
    pub show_non_cut: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { unit: Unit::Line, cut_pattern: DEFAULT_CUT_PATTERN.into(), memo_prefix: "▸".into(), show_non_cut: false }
    }
}

pub const DEFAULT_CUT_PATTERN: &str = r"^【(\d+)】";

/// 置ける 1 行（段落の単位なら 1 段落）
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// ファイルの行番号（1 始まり。段落なら最初の行）
    pub line: usize,
    pub cut: Option<String>,
    /// 置く本文（記法を外したもの。段落なら改行を含む）
    pub text: String,
    pub memos: Vec<String>,
}

impl Item {
    /// 印に使う名前。カット番号があればそれ、無ければ `L行番号`
    pub fn id(&self) -> String {
        self.cut.clone().unwrap_or_else(|| format!("L{}", self.line))
    }
}

/// 見出し 1 つと、その下の行
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// 見出しの `#` の数。0 は最初の見出しより前
    pub level: usize,
    pub title: String,
    pub line: usize,
    pub items: Vec<Item>,
}

impl Section {
    /// 見出しを置いたときの印の番号（`H行番号`）
    pub fn heading_id(&self) -> String {
        format!("H{}", self.line)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Manuscript {
    pub sections: Vec<Section>,
    /// カット番号に当たらず、一覧に出さなかった行の数
    pub hidden: usize,
}

impl Manuscript {
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.sections.iter().flat_map(|s| s.items.iter())
    }
}

static HR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:(?:\*[ \t]*){3,}|(?:-[ \t]*){3,}|(?:_[ \t]*){3,})$").unwrap());
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#{1,6})(?:[ \t]+(.*?))?[ \t]*$").unwrap());
static TABLE_DELIM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?$").unwrap());
static LIST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[-*+]|\d{1,9}[.)])[ \t]+").unwrap());
static TASK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[[ xX]\][ \t]+").unwrap());
static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(`{3,}|~{3,})").unwrap());

/// 原稿を読む。カット番号のパターンが正規表現として読めなければ Err
pub fn parse(text: &str, opts: &Options) -> Result<Manuscript, String> {
    let cut_re = if opts.cut_pattern.trim().is_empty() {
        None
    } else {
        Some(Regex::new(&opts.cut_pattern).map_err(|e| format!("カット番号のパターンが読めません: {e}"))?)
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();

    let mut ms = Manuscript::default();
    let mut section = Section { level: 0, title: String::new(), line: 0, items: Vec::new() };
    // 段落の単位で、まとめている途中のもの
    let mut para: Option<Item> = None;

    let mut i = 0;
    // YAML の前置き（1 行目が `---` のときだけ）
    if lines.first().is_some_and(|l| l.trim_end() == "---") {
        if let Some(end) = lines.iter().skip(1).position(|l| matches!(l.trim_end(), "---" | "...")) {
            i = end + 2;
        }
    }
    let mut fence: Option<String> = None;
    let mut in_html_comment = false;
    let mut in_obsidian_comment = false;

    while i < lines.len() {
        let no = i + 1;
        let raw = lines[i];
        i += 1;

        if let Some(open) = &fence {
            let t = raw.trim_start();
            if t.starts_with(open.as_str()) && t.trim_end().chars().all(|c| c == open.chars().next().unwrap_or('`')) {
                fence = None;
            }
            continue;
        }
        // コメントを外す（行をまたぐものは状態で持つ）
        let mut line = String::new();
        let mut rest = raw;
        loop {
            if in_html_comment {
                match rest.find("-->") {
                    Some(p) => {
                        rest = &rest[p + 3..];
                        in_html_comment = false;
                    }
                    None => break,
                }
            } else if in_obsidian_comment {
                match rest.find("%%") {
                    Some(p) => {
                        rest = &rest[p + 2..];
                        in_obsidian_comment = false;
                    }
                    None => break,
                }
            } else {
                let h = rest.find("<!--");
                let o = rest.find("%%");
                match (h, o) {
                    (Some(h), o) if o.is_none_or(|o| h < o) => {
                        line.push_str(&rest[..h]);
                        rest = &rest[h + 4..];
                        in_html_comment = true;
                    }
                    (_, Some(o)) => {
                        line.push_str(&rest[..o]);
                        rest = &rest[o + 2..];
                        in_obsidian_comment = true;
                    }
                    _ => {
                        line.push_str(rest);
                        break;
                    }
                }
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // コメントだけの行は空行と同じにしない（段落を切らない）
            if raw.trim().is_empty() {
                flush(&mut para, &mut section, &mut ms, opts);
            }
            continue;
        }
        if let Some(m) = FENCE.captures(trimmed) {
            flush(&mut para, &mut section, &mut ms, opts);
            fence = Some(m[1].to_string());
            continue;
        }
        if HR.is_match(trimmed) || TABLE_DELIM.is_match(trimmed) {
            flush(&mut para, &mut section, &mut ms, opts);
            continue;
        }
        if let Some(m) = HEADING.captures(trimmed) {
            flush(&mut para, &mut section, &mut ms, opts);
            let title = m.get(2).map(|t| t.as_str()).unwrap_or("");
            let title = title.trim_end_matches('#').trim_end();
            let prev = std::mem::replace(
                &mut section,
                Section { level: m[1].len(), title: strip_inline(title), line: no, items: Vec::new() },
            );
            if prev.level > 0 || !prev.items.is_empty() {
                ms.sections.push(prev);
            }
            continue;
        }

        let block = strip_block_prefix(trimmed);
        // メモの印は記法を外す前に見る（`` `▸` `` のようにコードの中に書いたものはメモにしない）
        if !opts.memo_prefix.is_empty() {
            if let Some(memo) = block.strip_prefix(opts.memo_prefix.as_str()) {
                let memo = strip_inline(memo);
                match para.as_mut().or(section.items.last_mut()) {
                    Some(item) => item.memos.push(memo),
                    None => ms.hidden += 1,
                }
                continue;
            }
        }
        // カット番号は書いたままの行に当てる（字下げ・リスト・太字の中の `【22】〜` は、判断の記録の中の参照で、カットではない）
        let (cut, rest) = take_cut(line.trim_end(), cut_re.as_ref());
        let rest = rest.trim();
        let body = strip_inline(strip_block_prefix(rest));
        if body.is_empty() && cut.is_none() {
            continue;
        }
        match opts.unit {
            Unit::Line => {
                let item = Item { line: no, cut, text: body, memos: Vec::new() };
                push(item, &mut section, &mut ms, opts);
            }
            Unit::Paragraph => match &mut para {
                Some(p) => {
                    p.text.push('\n');
                    p.text.push_str(&body);
                    if p.cut.is_none() {
                        p.cut = cut;
                    }
                }
                None => para = Some(Item { line: no, cut, text: body, memos: Vec::new() }),
            },
        }
    }
    flush(&mut para, &mut section, &mut ms, opts);
    if section.level > 0 || !section.items.is_empty() {
        ms.sections.push(section);
    }
    Ok(ms)
}

fn flush(para: &mut Option<Item>, section: &mut Section, ms: &mut Manuscript, opts: &Options) {
    if let Some(p) = para.take() {
        push(p, section, ms, opts);
    }
}

/// 一覧に足す。カット番号のパターンがあって当たらない行は、設定で出さない
fn push(item: Item, section: &mut Section, ms: &mut Manuscript, opts: &Options) {
    if item.cut.is_none() && !opts.cut_pattern.trim().is_empty() && !opts.show_non_cut {
        ms.hidden += 1;
        return;
    }
    section.items.push(item);
}

/// 行頭の引用・リスト・番号・チェックボックスを外す（`・` は本文の一部として残す）
fn strip_block_prefix(mut s: &str) -> &str {
    loop {
        let before = s;
        if let Some(r) = s.strip_prefix('>') {
            s = r.trim_start();
        }
        if let Some(m) = LIST.find(s) {
            s = &s[m.end()..];
        }
        if let Some(m) = TASK.find(s) {
            s = &s[m.end()..];
        }
        if s == before {
            return s;
        }
    }
}

/// カット番号を取り出し、本文から外す
fn take_cut(body: &str, re: Option<&Regex>) -> (Option<String>, String) {
    let Some(re) = re else { return (None, body.to_string()) };
    let Some(caps) = re.captures(body) else { return (None, body.to_string()) };
    let whole = caps.get(0).unwrap();
    let id = caps.get(1).unwrap_or(whole).as_str().trim().to_string();
    let rest = format!("{}{}", &body[..whole.start()], &body[whole.end()..]);
    (Some(id), rest.trim().to_string())
}

static EMBED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[\[[^\]]*\]\]|!\[[^\]]*\]\([^)]*\)").unwrap());
static WIKI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]|]*)(?:\|([^\]]*))?\]\]").unwrap());
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
static FOOTNOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\^[^\]]*\]").unwrap());
static EMPHASIS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*|__(.+?)__|~~(.+?)~~|==(.+?)==|\*([^*\s](?:[^*]*?))\*").unwrap());
static BR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br[ \t]*/?>").unwrap());
static BLOCK_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]\^[A-Za-z0-9-]+$").unwrap());
static ESCAPED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\([!-/:-@\[-`{-~])").unwrap());

/// 行の中の記法を外す。コードの中身（`` `…` ``）はそのまま残す
pub fn strip_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        match rest.find('`') {
            Some(open) => match rest[open + 1..].find('`') {
                Some(len) => {
                    out.push_str(&strip_inline_plain(&rest[..open]));
                    out.push_str(&rest[open + 1..open + 1 + len]);
                    rest = &rest[open + 1 + len + 1..];
                }
                None => {
                    out.push_str(&strip_inline_plain(rest));
                    break;
                }
            },
            None => {
                out.push_str(&strip_inline_plain(rest));
                break;
            }
        }
    }
    out.trim().to_string()
}

/// `\*` のようにエスケープした記号を、記法の処理の間だけ私用領域の文字に退避する
const STASH: u32 = 0xF0000;

fn strip_inline_plain(s: &str) -> String {
    let s = ESCAPED.replace_all(s, |c: &regex::Captures| {
        let ch = c[1].chars().next().unwrap();
        char::from_u32(STASH + ch as u32).unwrap().to_string()
    });
    let s = BLOCK_ID.replace(&s, "");
    let s = EMBED.replace_all(&s, "");
    let s = WIKI.replace_all(&s, |c: &regex::Captures| match c.get(2) {
        Some(alias) => alias.as_str().to_string(),
        None => {
            let target = &c[1];
            // `[[ノート#見出し]]` はノートの名前だけ（`#` から始まる同じノートの見出しなら見出し）
            match target.split_once('#') {
                Some(("", h)) => h.to_string(),
                Some((n, _)) => n.to_string(),
                None => target.to_string(),
            }
        }
    });
    let s = LINK.replace_all(&s, "$1");
    let s = FOOTNOTE.replace_all(&s, "");
    // 入れ子（`***太字斜体***`）のために、変わらなくなるまで繰り返す
    let mut s = s.into_owned();
    for _ in 0..4 {
        let next = EMPHASIS
            .replace_all(&s, |c: &regex::Captures| {
                (1..=5).find_map(|k| c.get(k)).map(|m| m.as_str().to_string()).unwrap_or_default()
            })
            .into_owned();
        if next == s {
            break;
        }
        s = next;
    }
    let s = BR.replace_all(&s, "\n");
    s.chars()
        .map(|c| match (c as u32).checked_sub(STASH) {
            Some(n) if n < 0x80 => char::from_u32(n).unwrap(),
            _ => c,
        })
        .collect()
}

// ---- 置いた印と照合 ----

const MARK_HEAD: &str = "<//原稿:";
const MARK_TAIL: &str = "//>";

/// 本文の末尾に付ける印（表示されないコメント。`aviutl2.txt` の「テキスト」の「コメント、文字列区切り」）。
/// 別の原稿の同じ番号と見分けるため、原稿のファイル名（拡張子なし）を入れる
pub fn mark_comment(stem: &str, id: &str) -> String {
    format!("{MARK_HEAD}{stem}:{id}{MARK_TAIL}")
}

/// 本文の中の印（ファイル名, 番号）を全部取る
pub fn parse_marks(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(p) = rest.find(MARK_HEAD) {
        let after = &rest[p + MARK_HEAD.len()..];
        let Some(end) = after.find(MARK_TAIL) else { break };
        // ファイル名に `:` は入らないので、最初の `:` で分ける
        if let Some((stem, id)) = after[..end].split_once(':') {
            out.push((stem.to_string(), id.to_string()));
        }
        rest = &after[end + MARK_TAIL.len()..];
    }
    out
}

/// 本文から印だけを外す（ほかのコメントは残す）
pub fn strip_marks(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(p) = rest.find(MARK_HEAD) {
        let after = &rest[p + MARK_HEAD.len()..];
        let Some(end) = after.find(MARK_TAIL) else { break };
        out.push_str(&rest[..p]);
        rest = &after[end + MARK_TAIL.len()..];
    }
    out.push_str(rest);
    out
}

/// 置いたテロップ 1 つが、原稿のどの行に当たったか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// `telops` の何番目か
    pub telop: usize,
    /// 印で当たった（偽なら本文の一致で当たった）
    pub by_mark: bool,
    /// 本文が今の原稿と同じか（印で当たって偽なら、原稿かテロップの片方だけが直されている）
    pub same_text: bool,
}

/// テロップの本文（保存形式を戻したもの）を原稿の行と見出しに当てる。戻り値は行の `id()`（見出しは `heading_id()`）→ 当たったテロップ。
/// 自分の原稿（`stem`）の印があればそれで、無ければ本文の一致で当てる（別の原稿の印が付いたものは当てない）
pub fn match_telops(ms: &Manuscript, stem: &str, telops: &[String]) -> HashMap<String, Vec<Hit>> {
    let mut entries: Vec<(String, &str)> = Vec::new();
    for sec in &ms.sections {
        if sec.level > 0 && !sec.title.is_empty() {
            entries.push((sec.heading_id(), sec.title.as_str()));
        }
        entries.extend(sec.items.iter().map(|i| (i.id(), i.text.as_str())));
    }
    let mut by_id: HashMap<&str, &str> = HashMap::new();
    let mut by_text: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, text) in &entries {
        by_id.entry(id.as_str()).or_insert(text.trim());
        by_text.entry(text.trim()).or_default().push(id.as_str());
    }
    let mut out: HashMap<String, Vec<Hit>> = HashMap::new();
    for (n, body) in telops.iter().enumerate() {
        let marks = parse_marks(body);
        let clean = strip_marks(body);
        let clean = clean.trim();
        let mine: Vec<&String> = marks.iter().filter(|(s, _)| s == stem).map(|(_, id)| id).collect();
        if !mine.is_empty() {
            for id in mine {
                if let Some(text) = by_id.get(id.as_str()) {
                    out.entry(id.clone()).or_default().push(Hit { telop: n, by_mark: true, same_text: *text == clean });
                }
            }
            continue;
        }
        if !marks.is_empty() || clean.is_empty() {
            continue;
        }
        if let Some(ids) = by_text.get(clean) {
            for id in ids {
                out.entry(id.to_string()).or_default().push(Hit { telop: n, by_mark: false, same_text: true });
            }
        }
    }
    out
}

/// 置く長さを「文字数 × 秒/字 + 余白」で決めるときの文字数。制御文字（`<…>`）は数えない。改行も数えない
pub fn visible_chars(text: &str) -> usize {
    let mut n = 0;
    let mut rest = text;
    while !rest.is_empty() {
        if rest.starts_with('<') {
            let close = if rest.starts_with("<//") {
                rest[3..].find("//>").map(|p| 3 + p + 3)
            } else if rest.starts_with("<?") {
                rest[2..].find("?>").map(|p| 2 + p + 2)
            } else {
                rest.find('>').map(|p| p + 1)
            };
            if let Some(len) = close {
                rest = &rest[len..];
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        if !c.is_whitespace() {
            n += 1;
        }
        rest = &rest[c.len_utf8()..];
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実際の原稿の書き方（【N】がカット、▸ が画面メモ、後半に尺の表）をまねた架空の原稿
    const SAMPLE: &str = "---\ntags: [原稿]\n---\n# Part 9「架空の回」原稿\n\n尺の目標: 3分\n\n**表記ルール**\n- `【N】` はカット番号\n- `▸` は画面メモ\n\n---\n\n## 導入(約20秒)\n\n【1】最初の文です。\n▸ フェードイン\n\n【2】通称「**架空**」。\n▸ タイトルとして大きく\n▸ 二つ目のメモ\n\n---\n\n## 前提(約15秒)\n\n【3】前提です。\n【4】・一つ目の条件\n【5】・二つ目の条件\n\n| 区間 | 秒 |\n|---|---|\n| 導入 | 20 |\n\n文中の【3】〜【5】の参照は行頭ではない。\n";

    fn opts() -> Options {
        Options::default()
    }

    #[test]
    fn reads_cuts_like_real_manuscript() {
        let ms = parse(SAMPLE, &opts()).unwrap();
        // 見出しは 3 つ（# と ## が 2 つ）。# の節にはカットが無い
        let titles: Vec<(usize, &str)> = ms.sections.iter().map(|s| (s.level, s.title.as_str())).collect();
        assert_eq!(titles, [(1, "Part 9「架空の回」原稿"), (2, "導入(約20秒)"), (2, "前提(約15秒)")]);
        let cuts: Vec<(Option<&str>, &str)> = ms.items().map(|i| (i.cut.as_deref(), i.text.as_str())).collect();
        assert_eq!(
            cuts,
            [
                (Some("1"), "最初の文です。"),
                (Some("2"), "通称「架空」。"),
                (Some("3"), "前提です。"),
                (Some("4"), "・一つ目の条件"),
                (Some("5"), "・二つ目の条件"),
            ]
        );
        // メモは直前のカットに付く
        assert_eq!(ms.sections[1].items[0].memos, ["フェードイン"]);
        assert_eq!(ms.sections[1].items[1].memos, ["タイトルとして大きく", "二つ目のメモ"]);
        // 行番号は 1 始まり
        assert_eq!(ms.sections[1].items[0].line, 16);
        // カットでない行（尺の目標・表記ルールの 3 行・表の 2 行・参照の行）は出さない
        assert_eq!(ms.hidden, 7);
    }

    #[test]
    fn shows_non_cut_lines_when_asked() {
        let o = Options { show_non_cut: true, ..opts() };
        let ms = parse(SAMPLE, &o).unwrap();
        assert_eq!(ms.hidden, 0);
        let first: Vec<&str> = ms.sections[0].items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(first, ["尺の目標: 3分", "表記ルール", "【N】 はカット番号", "▸ は画面メモ"]);
        // カットでない行の印は行番号
        assert_eq!(ms.sections[0].items[0].id(), "L6");
        // 行頭でない【3】には当たらない
        assert!(ms.items().any(|i| i.cut.is_none() && i.text.starts_with("文中の【3】")));
        // リストや太字の中の行頭の【22】も、カットではない（実物の原稿の後半の判断の記録）
        let ms = parse("【1】本文\n- 【22】【23】の生活への落とし込み\n- **【15】〜【16】が接続の要です。**\n  【52】の予告と\n", &opts()).unwrap();
        assert_eq!(ms.items().count(), 1);
        assert_eq!(ms.hidden, 3);
    }

    #[test]
    fn empty_pattern_lists_every_line() {
        let o = Options { cut_pattern: String::new(), ..opts() };
        let ms = parse("あ\nい\n▸ メモ\n", &o).unwrap();
        let t: Vec<&str> = ms.items().map(|i| i.text.as_str()).collect();
        assert_eq!(t, ["あ", "い"]);
        assert_eq!(ms.sections[0].items[1].memos, ["メモ"]);
        assert_eq!(ms.sections[0].level, 0);
    }

    #[test]
    fn bad_pattern_is_error() {
        let o = Options { cut_pattern: "【(".into(), ..opts() };
        assert!(parse("【1】あ", &o).is_err());
    }

    #[test]
    fn strips_markdown() {
        assert_eq!(strip_inline("**太字**と*斜体*と~~消し~~と==強調=="), "太字と斜体と消しと強調");
        assert_eq!(strip_inline("***両方***"), "両方");
        assert_eq!(strip_inline("[リンク](https://example.com/a_b)の先"), "リンクの先");
        assert_eq!(strip_inline("[[ノート|表示名]]と[[ノート2]]と[[ノート3#見出し]]"), "表示名とノート2とノート3");
        assert_eq!(strip_inline("画像![図](a.png)と![[b.png]]と脚注[^1]"), "画像とと脚注");
        assert_eq!(strip_inline("`**コード**`の中は残す"), "**コード**の中は残す");
        assert_eq!(strip_inline("1行目<br>2行目"), "1行目\n2行目");
        assert_eq!(strip_inline("ブロック ID ^abc-1"), "ブロック ID");
        assert_eq!(strip_inline(r"\*記号\*そのもの"), "*記号*そのもの");
        // `_` だけの斜体は外さない（ファイル名の Part1_原稿 を壊さない）
        assert_eq!(strip_inline("Part1_原稿_叩き台"), "Part1_原稿_叩き台");
        // 3 * 4 のような空白つきの `*` は斜体にしない
        assert_eq!(strip_inline("3 * 4 * 5"), "3 * 4 * 5");
    }

    #[test]
    fn keeps_aviutl2_control_characters() {
        let ms = parse("【1】今日は<$強調>3 つ<$>に<#2B1B4A>絞ります<#>。", &opts()).unwrap();
        assert_eq!(ms.sections[0].items[0].text, "今日は<$強調>3 つ<$>に<#2B1B4A>絞ります<#>。");
    }

    #[test]
    fn strips_block_prefixes() {
        let o = Options { cut_pattern: String::new(), ..opts() };
        let ms = parse("- リスト\n* 星\n1. 番号\n2) 括弧\n> 引用\n> - 引用の中のリスト\n- [ ] 未完\n- [x] 完了\n", &o).unwrap();
        let t: Vec<&str> = ms.items().map(|i| i.text.as_str()).collect();
        assert_eq!(t, ["リスト", "星", "番号", "括弧", "引用", "引用の中のリスト", "未完", "完了"]);
    }

    #[test]
    fn skips_comments_code_and_front_matter() {
        let o = Options { cut_pattern: String::new(), ..opts() };
        let text = "---\ntitle: x\n---\n残る1\n<!-- 消える -->\n前<!-- 中 -->後\n<!--\n複数行\n-->\n%%消える%%\n%%\n複数行の\nObsidian コメント\n%%\n```lua\nコード\n```\n~~~\nコード2\n~~~\n***\n___\n残る2\n";
        let ms = parse(text, &o).unwrap();
        let t: Vec<&str> = ms.items().map(|i| i.text.as_str()).collect();
        assert_eq!(t, ["残る1", "前後", "残る2"]);
        // 行番号は前置きを数えたまま
        assert_eq!(ms.sections[0].items[0].line, 4);
    }

    #[test]
    fn front_matter_only_at_first_line() {
        let o = Options { cut_pattern: String::new(), ..opts() };
        let ms = parse("本文\n---\n次\n---\n", &o).unwrap();
        let t: Vec<&str> = ms.items().map(|i| i.text.as_str()).collect();
        assert_eq!(t, ["本文", "次"]);
    }

    #[test]
    fn crlf_and_bom() {
        let ms = parse("\u{feff}## 見出し\r\n\r\n【1】あ\r\n", &opts()).unwrap();
        assert_eq!(ms.sections[0].title, "見出し");
        assert_eq!(ms.sections[0].items[0].text, "あ");
    }

    #[test]
    fn paragraph_unit_keeps_newlines() {
        let o = Options { unit: Unit::Paragraph, ..opts() };
        // 【5】【6】【7】のように空行を挟まずに続くカットは 1 つにまとまる（実物の原稿で段落を既定にしなかった理由）
        let ms = parse("【1】一行目\n二行目\n▸ メモ\n\n【5】あ\n【6】い\n【7】う\n", &o).unwrap();
        let items: Vec<(Option<&str>, &str)> = ms.items().map(|i| (i.cut.as_deref(), i.text.as_str())).collect();
        assert_eq!(items, [(Some("1"), "一行目\n二行目"), (Some("5"), "あ\nい\nう")]);
        assert_eq!(ms.sections[0].items[0].memos, ["メモ"]);
    }

    #[test]
    fn cut_without_group_uses_whole_match() {
        let o = Options { cut_pattern: r"^C-\d+".into(), ..opts() };
        let ms = parse("C-001 ここが一つ目？\n", &o).unwrap();
        assert_eq!(ms.sections[0].items[0].cut.as_deref(), Some("C-001"));
        assert_eq!(ms.sections[0].items[0].text, "ここが一つ目？");
    }

    #[test]
    fn marks_round_trip() {
        let m = mark_comment("Part1_原稿_叩き台", "12");
        assert_eq!(m, "<//原稿:Part1_原稿_叩き台:12//>");
        let body = format!("本文<// 手で書いたコメント //>{m}");
        assert_eq!(parse_marks(&body), [("Part1_原稿_叩き台".to_string(), "12".to_string())]);
        assert_eq!(strip_marks(&body), "本文<// 手で書いたコメント //>");
        // 閉じない印はそのまま
        assert_eq!(strip_marks("あ<//原稿:x:1"), "あ<//原稿:x:1");
    }

    #[test]
    fn matches_by_mark_then_text() {
        let ms = parse("【1】一つ目\n【2】二つ目\n【3】三つ目\n", &opts()).unwrap();
        let telops = vec![
            format!("一つ目{}", mark_comment("P", "1")),
            // 印があって本文だけ違う（原稿を直した後）
            format!("古い二つ目{}", mark_comment("P", "2")),
            // 印なし・本文が同じ
            "三つ目".to_string(),
            // 別の原稿の印が付いたものは、本文が同じでも当てない
            format!("一つ目{}", mark_comment("Q", "1")),
            // どれにも当たらない
            "関係ない".to_string(),
        ];
        let hits = match_telops(&ms, "P", &telops);
        assert_eq!(hits["1"], [Hit { telop: 0, by_mark: true, same_text: true }]);
        assert_eq!(hits["2"], [Hit { telop: 1, by_mark: true, same_text: false }]);
        assert_eq!(hits["3"], [Hit { telop: 2, by_mark: false, same_text: true }]);
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn matches_headings() {
        let ms = parse("## 導入\n\n【1】一つ目\n", &opts()).unwrap();
        assert_eq!(ms.sections[0].heading_id(), "H1");
        let telops = vec![format!("導入{}", mark_comment("P", "H1")), "導入".to_string()];
        let hits = match_telops(&ms, "P", &telops);
        assert_eq!(
            hits["H1"],
            [Hit { telop: 0, by_mark: true, same_text: true }, Hit { telop: 1, by_mark: false, same_text: true }]
        );
    }

    /// 手元の原稿を読めるか。原稿は写さず、その場で読むだけ。フォルダは環境変数で渡す:
    /// `MANUSCRIPT_DIR=<原稿のフォルダ> cargo test -- --ignored reads_real_manuscripts --nocapture`
    /// フォルダの `.md` ごとに、カット番号が 1 から連番かを確かめ、カットとメモの数を出す。
    /// `MANUSCRIPT_EXPECT=カット数,メモ数` を渡すと、合計がその値かも確かめる
    #[test]
    #[ignore]
    fn reads_real_manuscripts() {
        let dir = std::env::var("MANUSCRIPT_DIR").expect("MANUSCRIPT_DIR に原稿のフォルダを渡す");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .collect();
        files.sort();
        let (mut cuts, mut memos, mut counted) = (0, 0, 0);
        for p in &files {
            let ms = parse(&std::fs::read_to_string(p).unwrap(), &Options::default()).unwrap();
            let c = ms.items().count();
            if c == 0 {
                continue;
            }
            let m: usize = ms.items().map(|i| i.memos.len()).sum();
            let name = p.file_name().unwrap().to_string_lossy();
            let ids: Vec<usize> = ms.items().map(|i| i.cut.as_ref().unwrap().parse().unwrap()).collect();
            assert_eq!(ids, (1..=c).collect::<Vec<_>>(), "{name} の番号が連番でない");
            println!("{name}: カット {c} / メモ {m} / 出さなかった行 {} / 見出し {}", ms.hidden, ms.sections.len());
            cuts += c;
            memos += m;
            counted += 1;
        }
        println!("合計 {counted} 本: カット {cuts} / メモ {memos}");
        assert!(counted > 0, "カットのある原稿が無い");
        if let Ok(expect) = std::env::var("MANUSCRIPT_EXPECT") {
            assert_eq!(format!("{cuts},{memos}"), expect);
        }
    }

    #[test]
    fn counts_visible_chars() {
        assert_eq!(visible_chars("今日は<$強調>3 つ<$>に絞ります。"), 11);
        assert_eq!(visible_chars("あ<//原稿:P:1//>"), 1);
        assert_eq!(visible_chars("一\n二"), 2);
        // 閉じない < は文字として数える
        assert_eq!(visible_chars("a<b"), 3);
    }
}
