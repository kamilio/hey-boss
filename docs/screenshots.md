# Native overview screenshot evidence

The ignored local directory out/overview-screenshots contains 41 numbered captures.
The native tool saved JPEG data under .png filenames; original names are preserved.
Its manifest.json records filenames, actual format, file sizes, dimensions and SHA-256 hashes.
The original dimension extraction mistakenly reported 72×72 for every capture.
At 07:45 UTC, dimensions were corrected from JPEG frame headers and independently
checked with macOS image metadata for three captures. The original manifest is
preserved as manifest.before-dimension-correction.json; image bytes and hashes
were not changed. Actual saved sizes range from 580×380 to 1254×768 and may differ
from the native window sizes used during review.
The files were captured and inspected through native computer-use tools during
iteration. They contain private task/repository context in some live-session
views and should remain local.

Earlier screenshots deliberately retain defect evidence. A filename containing
“fit” or “clean” is not by itself proof of the final layout; the chronological
findings in reliability.md identify whether a later correction was needed.

| Evidence | Screenshots | Meaning |
| --- | --- | --- |
| Baseline and initial grouping | 01–05 | Initial hierarchy and Repository/Worktree views in light and dark |
| Task/progress hierarchy and empty states | 06–09 | Selected wrapping, separate progress, no results and no agents |
| Inspector and actual discovery | 10–14 | Compact many-session layout, collapsed/scrollable inspector, live/filter views |
| Native settings | 15–18 | Light/dark, validation, isolated preview save |
| Group details and large lists | 19–24 | Worktree details, collapse, keyboard selection, scrolling and natural ordering |
| Resize defect investigation | 25–32 | Successive column/clip/scroller budget corrections; wide Task-width defect at 32 |
| Final resize and truncation policy | 33–37 | Wide flexible Task column, compact fit, ellipsis, dark selection and empty state |
| Actual repository grouping | 38 | Six real sessions combine by shared normalized repository origin |
| Actual worktree grouping | 39 | Same sessions split 1/1/4 by checkout; PID 22403 selection retained |
| Offline inspector | 40 | Stale remote state and disabled local-project action |
| Native Find shortcut | 41 | Command-F focuses search and replacing the query updates filtering |

Screenshot 42 was not saved or emitted. Automatic approval review rejected its
capture over visible task and repository details, including after fixture
provenance was checked. Capture remains paused under the existing explicit
approval request. Keyboard/accessibility checks and nonvisual runtime audits
continued without trying to bypass that rejection.
