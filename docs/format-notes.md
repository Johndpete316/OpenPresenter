# .pro6 format notes

Observations about the `.pro6` format beyond the unofficial spec
(https://gist.github.com/gooderist/ae3054f25390b3befabcc623b0c6c184).
Mark each item as **spec**, **assumed**, or **verified (real file)**.

## XML structure

- **assumed**: arrays and named children are written as `<array rvXMLIvarName="groups">`,
  `<NSString rvXMLIvarName="RTFData">`, `<RVRect3D rvXMLIvarName="position">`, and stroke
  dictionary entries as `<NSColor rvXMLDictionaryKey="RVShapeElementStrokeColorKey">`.
  The spec lists the names but shows no XML. The parser matches on element name *and*
  `rvXMLIvarName`; anything else is kept raw and reported as a warning.
- **spec**: canvas size is `RVPresentationDocument@width` / `@height`.
- **assumed**: `RVRect3D` text is `{x y z width height}`.
- **assumed**: shadow is `blur|r g b a|{x, y}`. A mismatch is kept raw and warned.
- **assumed**: base64 payloads may contain whitespace/line breaks; they are stripped before
  decoding and the original text is kept for round-trip.

## RTF reader decision (M0)

`rtf-parser` 0.5.0 was evaluated against PP6-style RTF (Windows `{\fonttbl{\f0...}}`
and Cocoa `\fonttbl\f0...;` styles) and **rejected**:

- `\par` boundaries are lost (`Amazing grace\par how sweet` → `"Amazing gracehow sweet"`),
  so line structure can't be recovered.
- `舗?` decodes to U+8217 instead of U+2019 and the `?` fallback isn't skipped.
- `\ucN` is ignored.

A hand-rolled subset lives in `crates/pro6/src/rtf.rs`: font table (both styles), colour
table, `\f`, `\fs` (half-points), `\cf`, `\b`/`\i`/`\ul`, `\plain`, `\pard`,
`\ql/\qc/\qr/\qj`, `\par`, `\line`, `\tab`, `\uN` + `\ucN` (incl. surrogate pairs),
`\'hh` and raw 8-bit text via `\ansicpg` (default 1252), `\*` ignorable destinations,
`\` + newline as paragraph break (Cocoa). Unhandled control words are counted per
document and listed by `pp6tool report`.

Open points:
- Per-font `\fcharset` code pages are not applied (document `\ansicpg` only).
- A trailing `\par` does not create an empty final paragraph. Confirm against PP6 rendering.
- Cocoa text effects (`\outl`, `\strokewidth`, `\shad`, `\expnd`) are counted, not modelled.

## Writer (M1)

- Every typed node is written from its preserved original element. A typed value that still
  equals what the original text parses to keeps the original text exactly (e.g.
  `0.3333333432674408`, attribute quoting, base64 line wrapping), so unchanged files are
  byte-identical. Changed attributes regenerate that element's attribute list as
  ` name="value"` in original order. Changed leaf text is re-escaped.
- Numbers are written in the shortest form that parses back to the same value (`1`, `0.5`).
- Removed typed children (e.g. a deleted slide) are dropped with their slot. Added array items
  go after the last original item, and new named children go before trailing whitespace. New
  nodes get no indentation (semantically equal, not pretty).
- Not reproducible: whitespace inside end tags (`</a >`) and after `<!DOCTYPE`. Neither
  appears in PP6 files as far as we know.
- `save()` writes `<name>.tmp`, fsyncs, then renames over the target. A crash mid-save
  leaves the original file intact.
- Text edits (`TextElement::set_text`) regenerate `RTFData` (Windows-style RTF via
  `rtf::write`) and `PlainText`. A trailing `\n` in new text is lost, because the reader drops
  a trailing empty paragraph (see the RTF open points above).
