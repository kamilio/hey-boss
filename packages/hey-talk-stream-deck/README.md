# Hey Talk for Stream Deck

Open the versioned `.streamDeckPlugin` installer on a Mac with Hey Talk and Stream Deck 6.6 or later. The Mini gets an editable **Hey Talk** profile with two pages. On other decks, drag the actions from **Hey Talk** to the keys you want. To update, open a newer installer; the plugin and action IDs stay the same.

The keys follow the pill: indigo waveform at rest, teal bars for microphone levels, purple for Auto, a rim while transcribing, and red brain and Return badges when correction or submission is off. Conversation uses its own microphone, pause, speaker, and approval icons. A disconnected mark means Hey Talk is offline; pressing a key opens it without activating its window. An amber update arrow means the installed Hey Talk app needs the live-key protocol.

| Keyboard | Stream Deck action |
| --- | --- |
| Hyper+R | Dictation, including switching out of a conversation and back |
| Hyper+C | Conversation; press again for the pill’s current action |
| Hyper+V | Auto Dictate |
| Hyper+S / Hyper+T | Playback / Read Aloud |
| Hyper+M | Catch Up |
| Enter during Auto recording / Esc | Send Now / Stop |

The first page has Dictation, Conversation, Stop, Auto Dictate, Catch Up and Next. The second has Chat Mic, Next Turn, Transcript and Back. Playback, Read Aloud, Send Now and New Chat can be added from the editor.

**Build and install:** Run `../hey-talk/install.sh` first when updating the native service. With Node 22 and the macOS zip utility, run `npm ci && npm test && npm run install:deck`. For redistribution, `npm run pack` validates the manifest and writes the installer to `dist/`. CI produces the same installer on package changes. To regenerate the state gallery and PNG sheets, install librsvg and run `npm run preview`.

One local socket subscription serves all visible keys. Hey Talk transmits changed indicator frames only; Stream Deck redraws only affected keys. Hiding the last key closes the subscription. No transcripts, window titles or audio cross that connection.
