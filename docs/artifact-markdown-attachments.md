# Markdown attachments

Creating or editing an artifact from the CLI rehosts local image and file links.
Paths in `--file report.md` resolve beside that Markdown file; paths in `--body`
or stdin resolve from the current directory. Absolute paths and `file:` URLs
also work. External URLs and heading links stay unchanged. Code examples are
never uploaded.

In the artifact editor, choose **Import**, then select one Markdown document and
its linked files together. Preview includes the selected images; **Save** uploads
them with the document. Browsers cannot read neighboring files automatically:
a missing or ambiguous filename is reported before the draft changes.

Inline links, images, and reference definitions retain their text and titles.
Repeated references share an uploaded file. Each import supports up to 10 MiB of
file contents in total. A failed upload or stale document revision leaves the
saved document unchanged. Retrying the same request does not duplicate files.

Hosted images render through the authenticated attachment service on desktop and
paired phones. File links download the original bytes and filename. Attached
files remain available after the source files are moved or deleted.
