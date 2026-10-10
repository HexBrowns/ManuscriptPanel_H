# ManuscriptPanel_H（原稿パネル）

Markdown の原稿を行ごとに一覧にし、クリックした行を、決めた見た目（型 = エイリアス）のテキストオブジェクトとしてカーソル位置に置く AviUtl2 の汎用プラグイン。
置いた行には印を付け、「照合」でプロジェクトの中身から印を付け直す。

- 仕様: `AI/specifications/20261005_ManuscriptPanel_H_spec.md`
- 本体 2.1.11 以降（aviutl2-rs 0.48）

## ビルドと配置

```bash
python AI/tools/au2_build.py ManuscriptPanel_H                     # 本番へ
python AI/tools/au2_build.py ManuscriptPanel_H --deploy-root=C:/ProgramData/aviutl2/AI/devenv/aviutl2/data   # devenv へ
MANUSCRIPT_DIR=<原稿のフォルダ> cargo test -- --ignored reads_real_manuscripts --nocapture   # 手元の原稿を読めるか（写しは置かない）
```

## 作りの要点

| ファイル | 中身 |
|---|---|
| `src/manuscript.rs` | 原稿 → 行の一覧（記法を外す・カット番号・画面メモ・段落）、置いた印の書き方と照合（cargo test の中心） |
| `src/place.rs` | 型（エイリアス）の検査と本文の差し込み、置く（`call_edit_section` 1 回）、表示中のシーンのテキストの読み取り |
| `src/aup2.rs` | 保存済みの `.aup2` からテキストの本文を読む（表示中でないシーンの照合用。TextReplace_H の写しを絞ったもの） |
| `src/watcher.rs` | 原稿の監視（`notify`）。保存されたら UI が読み直す |
| `src/dialog.rs` | 原稿を選ぶダイアログ |
| `src/settings.rs` | 設定（`Plugin/ManuscriptPanel_H/設定.json`） |
| `src/gui.rs` | ウィンドウ（egui） |

- 本体への書き込みは、行のクリックとボタンのときだけ。ファイルの変化やフォーカスの変化では書かない
- 本文は型のエイリアスの `テキスト=` を書き換えてから作る（作った後に書き換えない）。作った後に本文を読み返して確かめる。
  本文は保存形式にしてから入れる（`\` → `\\`、改行 → `\n`。TextReplace_H と同じ）
- 置く場所が 1 つでも埋まっていたら何も作らない（本体は長さ 0 だと位置をずらして置くので、長さを決めて、空きを先に確かめる）
- カット番号は書いたままの行の頭に当てる。字下げ・リスト・太字の中の `【22】〜` は、原稿の後半の判断の記録の中の参照なので当てない
  （手元の原稿 7 本で確かめた）
- 印は `<//原稿:ファイル名:番号//>`（表示されないコメント）。別の原稿の同じ番号と見分けるためにファイル名を入れる。
  照合は、自分の原稿の印があればそれで、無ければ本文の一致で当てる。印で当たって本文が違えば黄色の印にする
- 原稿の場所はプロジェクトの中に覚える（`ProjectFile::set_param_string`。保存のときに書き、開いたときに読む）
- 第 2 段階（v0.2.0）: 見出しの型（印は `H行番号`）、本文がずれたテロップを原稿の文に直す（`place::rewrite`。書く直前に読み直す）、節をまとめて置く（型は 1 つずつ `Piece` に持つ）
- 自分ではスレッドを起こさない。`notify` の中のスレッドとシェルのダイアログは待てないので DLL を pin し、監視はプラグインの `Drop` で外す
