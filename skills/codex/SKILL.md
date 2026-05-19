---
name: rr
description: >
  Push markdown files and documents to a reMarkable tablet using the rr CLI tool.
  Use when the user wants to save content to reMarkable, sync files, upload notes,
  create folders, or organize documents on their reMarkable device.
  Triggers on mentions of "remarkable", "rr", "push to tablet", "save to remarkable",
  "send to remarkable", "sync with remarkable", "upload to tablet", or similar.
---

# rr — Push Content to reMarkable Tablet

The `rr` CLI is a Rust-based tool that syncs markdown files to a reMarkable
Paper Pro tablet via the cloud API. It converts markdown to PDF and uploads
it, or creates native reMarkable notebook folders.

**Installation:** Already built at `~/remarkable_rust/rr`

**Config location:** `~/Library/Application Support/rr/config.toml` (macOS)

---

## Trigger Conditions

Activate this skill when the user:

- Says "push this to remarkable", "send to tablet", "save for remarkable"
- Mentions "rr" in context of document upload/sync
- Wants to create a folder or organize documents on their reMarkable
- Says "upload these notes" or "sync this file"
- Wants to check what's already on their reMarkable
- Mentions "remarkable" in context of file transfer or document management

---

## First-Time Setup

If `rr auth` has never been run, the user needs to pair their device:

```bash
# Run in terminal (requires user interaction)
~/remarkable_rust/rr auth
```

**What happens:**
1. Agent displays a one-time code (e.g., `ABCD-EFGH`)
2. User goes to `https://my.remarkable.com` and enters the code
3. Agent automatically exchanges for access token
4. Token saved to config file

**The agent should NEVER run `rr auth` automatically.** Always ask the user
to run it manually in their terminal if authentication fails.

---

## Commands

### Authentication & Status
```bash
rr auth                    # Pair device (interactive, user must run)
rr ls                      # List all documents in cloud
rr ls | grep "folder"      # Search for specific documents
```

### Upload
```bash
rr upload file.md                    # Upload as PDF to root
rr upload file.md --folder "Notes"   # Upload into existing/new folder
rr upload file.md --dir "Work/2024"   # Create nested path and upload
```

**Important:** Uploads are one-way (local → cloud). There's no automatic sync.
Each `rr upload` creates a NEW document in the cloud. If you update a local
file and re-upload, you'll get a duplicate unless you delete the old one first.

### Folders
```bash
rr mkdir "Folder Name"        # Create folder at root
```

---

## Agent Workflows

### 1. Push a Summary or Notes

When the user says "send this to my remarkable" or similar:

```bash
# 1. Check if authenticated
~/remarkable_rust/rr ls > /dev/null 2>&1
# If error: ask user to run 'rr auth' first

# 2. Generate a markdown file from the conversation
# (The agent should create a .md file with the summary)

# 3. Pick a descriptive filename
cat > /tmp/airwallex-negotiation-2026-05-20.md << 'EOF'
# Airwallex Negotiation Notes

## Key Decisions
- Pricing tier: Enterprise
- Volume commitment: 10K txs/month
...
EOF

# 4. Upload it
~/remarkable_rust/rr upload /tmp/airwallex-negotiation-2026-05-20.md --folder "AI Notes"
```

### 2. Check Before Uploading (Avoid Duplicates)

```bash
# List existing documents to avoid creating duplicates
~/remarkable_rust/rr ls

# If a document with similar name exists, ask user:
# "You already have 'Airwallex Notes' on remarkable. Overwrite or create new?"
```

**Note:** `rr` doesn't have an update-in-place command. To "overwrite":
1. The user would need to manually delete the old doc on the device
2. Then re-upload the new version

### 3. Create Organized Folder Structure

```bash
# Create folders for different types of content
~/remarkable_rust/rr mkdir "AI Notes"
~/remarkable_rust/rr mkdir "Projects"
~/remarkable_rust/rr mkdir "Meeting Notes"

# Upload into specific folder
~/remarkable_rust/rr upload notes.md --folder "Meeting Notes"
```

### 4. Upload Multiple Files

```bash
# Create nested directory structure
~/remarkable_rust/rr upload report.md --dir "Projects/Q2-2024"

# This creates:
#   Projects/ (folder)
#     └── Q2-2024/ (folder)
#           └── report.pdf (document)
```

---

## Error Handling

### "401 Unauthorized" or "token expired"
```
rr ls
# Error: token expired or invalid
```
**Action:** Tell user: "Your reMarkable token expired. Please run `rr auth` to re-pair."

### "Device not registered" or "no token found"
**Action:** Tell user: "You need to authenticate first. Run `~/remarkable_rust/rr auth` in your terminal."

### "Document already exists" (not an actual error)
`rr` allows duplicates. If user wants to avoid duplicates, they should:
1. Check existing docs with `rr ls`
2. Manually delete old version on device
3. Re-upload

### Network errors
**Action:** Retry once after 2 seconds. If still failing, report to user.

---

## Content Generation Rules

When creating markdown to upload:

1. **Use descriptive filenames** — `airwallex-negotiation-2026-05-20.md` not `notes.md`
2. **Include metadata** — Date, topic, key decisions at the top
3. **Structure for reading** — Use headings, bullet points, tables
4. **Keep it concise** — 1-3 pages is ideal for reMarkable
5. **No emojis** — They may not render well in PDF conversion

---

## Examples

**User says:** "Send these meeting notes to my remarkable"
**Agent does:**
```bash
# Create markdown from meeting content
cat > /tmp/meeting-notes.md << 'EOF'
# Team Sync — May 20, 2026

## Action Items
- [ ] Hitesh: Follow up with Airwallex
- [ ] Team: Review Q2 targets

## Decisions
- Moving to weekly sprints
- Hiring 2 more engineers
EOF

# Upload
~/remarkable_rust/rr upload /tmp/meeting-notes.md --folder "Meeting Notes"
```

**User says:** "Push this thread summary to remarkable"
**Agent does:**
```bash
# Check existing docs first
~/remarkable_rust/rr ls | grep -i "thread" || true

# Create summary
cat > /tmp/thread-summary.md << 'EOF'
# Thread Summary: [Topic]

## Key Points
...
EOF

# Upload to default folder
~/remarkable_st/rr upload /tmp/thread-summary.md --folder "AI Notes"
```

**User says:** "What do I have on my remarkable?"
**Agent does:**
```bash
~/remarkable_rust/rr ls
```

---

## Limitations

- **One-way sync only** — Uploads go local → cloud. No automatic download.
- **No update-in-place** — Re-uploading creates a new document.
- **PDF format** — Markdown is converted to PDF before upload.
- **No native notebook editing** — Cannot edit handwritten notes from the device.
- **Requires manual auth** — The `rr auth` command needs user interaction.

---

## Related

- `/remarkable` — Summarize conversation and publish as print-ready HTML
- `remarkable-cli` — Full CLI for reMarkable management (SSH + Cloud)
