# Connect-free account test

If you can read this on your device, **`rr self-push`** works on any
reMarkable account — no Connect subscription required. The pipeline
uses the same sync v3 API every device speaks to keep notebooks in
sync; it's part of owning a reMarkable, not a paid add-on.

## What this proves

- Native v6 notebook bytes shipped from a laptop to a non-Connect
  account land on the device through the standard cloud sync path.
- Multi-page works
- Image embedding works

---

## A small table

| Tier        | Notebook sync | Read-on-rM | rr self-push |
|-------------|---------------|------------|--------------|
| Free        | yes           | no         | **yes**      |
| Connect     | yes           | yes        | yes          |

Both rows above should render as the same kind of native typed-text
page, regardless of which account is logged in.
