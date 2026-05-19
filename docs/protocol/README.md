# Read-on-reMarkable protocol notes

The reMarkable cloud has two layers:

| Layer | Used by | Endpoint shape |
|-------|---------|----------------|
| **Document API** (`doc/v2`, `import/v1`) | Read-on-reMarkable Chrome ext, web app | High-level: "upload this EPUB", server handles indexing + conversion |
| **Sync API** (`sync/v3`) | The tablet itself | Low-level: hash-addressed blob store + root pointer |

This CLI was originally reverse-engineered from the Chrome extension
("Read on reMarkable", id `bfhkfdnddlhfippjbflipboognpdpoeh`) and uses the
**Document API** — the same path the extension takes when you press the
extension button on a web page. The sync-v3 path was a detour that ended up
shipping local-PDF uploads, which is *not* what we want: the device renders
those as PDFs rather than as native reMarkable documents.

## Base URL resolution

The user's JWT carries a `tectonic` claim (`eu`, `us`, …). The base URL is:

```
https://web.{tectonic}.tectonic.remarkable.com
```

Fallback when the claim is missing: `https://internal.cloud.remarkable.com`.

## Endpoints

All endpoints share these headers:

```
Authorization: Bearer <user_token>
rM-Source:     RoR-Browser     # or any opaque tag; the extension uses this
rM-Meta:       base64(JSON({...customFields, file_name}))
```

### Upload (EPUB → native reMarkable doc)

```
POST /doc/v2/files
Body: raw EPUB bytes (application/epub+zip)
Meta: { parent: "<folder-id-or-empty>", orientation: "portrait" }
```

### Import (EPUB → notebook via server conversion)

```
POST /import/v1/files
Body: raw EPUB bytes
Meta: { parent, orientation: "portrait", convert: true }
```

The extension calls this for `FileFormat.NOTEBOOK`; the server converts the
EPUB into a `.notebook` (the document type shown with the yellow icon).

### Create folder

```
POST /doc/v2/files
Body: ""
Headers: Content-Type: folder
Meta:    { parent: "<parent-or-empty>", file_name: "<name>" }
```

### List

```
GET /doc/v2/files[?onlyFolders=true]
If-None-Match: <etag>   # optional, returns 304 if unchanged
```

Response: `{ "files": [ { id, hash, type, file_name, parent, ... } ] }`

### Delete

```
DELETE /doc/v2/files
Body: { "hashes": ["...", "..."] }
```

### Update metadata (rename, move, pin, …)

```
PATCH /doc/v2/files/<id>          # single
PATCH /doc/v2/files               # multi: { hashes, updates }
```

## EPUB layout

The extension builds a minimal EPUB 3 zip:

```
mimetype                    (stored, not compressed, must be first)
META-INF/container.xml
OEBPS/content.opf
OEBPS/nav.xhtml
OEBPS/article.xhtml
OEBPS/<image>.{jpg,png}*
```

`content.opf` carries dublin-core metadata (title, creator, language, date,
source URL) and a manifest listing every file plus a single-itemref spine.

## Error model

| HTTP | Meaning |
|------|---------|
| 401  | Token expired → refresh via device token, retry once |
| 403  | Sub not active |
| 409  | Conflict (rare; same hash already uploaded) |
| 429  | Rate-limited; honor `Retry-After` |

## Source

The TypeScript files mirrored next to this README are extracted from the
extension's source maps (it ships them, the developers were generous with
us). They are NOT shipped in builds — they exist only as reference. Update
them whenever the extension version bumps and the protocol changes.
