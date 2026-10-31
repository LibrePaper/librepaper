# German glossary (de)

## 1. Register and conventions

- Address: formal "Sie" (Sie, Ihr, Ihnen, always capitalized). Reason: the audience is academics of every age, and Microsoft, Google Docs and most academic software address German users with "Sie". Avoid addressing the user where a neutral sentence works ("Das Projekt wurde gespeichert."). This overrides any "du" form elsewhere in this glossary.
- Buttons and menu items: infinitive or noun, never imperative. Examples: "Speichern", "Datei öffnen", "Projekt teilen", "Abbrechen". Commands in command-palette entries follow the same rule.
- Messages: full sentences end with a period. Error messages say what failed, in the passive or with "konnte nicht": "Die Datei konnte nicht gespeichert werden."
- Gender: use neutral wording first (for example "Mitglieder", "Personen", "Konto"). Where a role noun is unavoidable, use the generic masculine in short labels as mainstream software does (see the role rows below). Do not use gender stars, colons or slashes.
- Quotation marks: German low-high „…“ (U+201E and U+201C). Nested: ‚…‘. Never use straight quotes in translated prose. Placeholders such as "{query}" in the English source become „{query}“.
- Ellipsis: use the single character … (U+2026) with no space before it, exactly where English has it ("Datei auswählen…").
- Spacing: a normal space before ":" is wrong; write "Name:" with no space. Put a no-break space (U+00A0) between a number and its unit ("12 MB", "5 Min.") and in "z. B.", "d. h.", "u. a.".
- Decimal comma and thousands separator: handled by number formatting; in hand-written text use "1,5 GB".
- Capitalization: nouns are capitalized. Labels are in sentence style ("Neues Projekt"), not title case.
- Compounds: write closed compounds ("Projektordner", "Hauptdatei", "Vorschaufenster"). Use a hyphen only for clarity or with acronyms and product names ("PDF-Vorschau", "Git-Repository", "LaTeX-Paket", "DOCX-Datei").
- Long strings: German runs 20 to 40 percent longer. Prefer the shorter correct term for labels and buttons (for example "Löschen" not "Entfernen", "Konto" not "Benutzerkonto", "Teilen" not "Freigeben").
- Keep placeholders such as {count}, {path}, {who} unchanged and in a position that reads correctly. For plurals, German has two forms (one and other).

## 2. Menu titles

| English | German | Note |
|---|---|---|
| File | Datei | |
| Edit | Bearbeiten | Microsoft and Apple both use this verb form for the menu |
| Insert | Einfügen | |
| View | Ansicht | |
| Settings | Einstellungen | |
| Help | Hilfe | |
| Tools | Werkzeuge | |
| Format | Format | |

Menu paths in sentences use the same words with " > ": "Ansicht > PDF", "Datei > Als Vorlage speichern", "Einstellungen > Backups". Use a plain ">" with a space on each side. The menu item after the arrow must match the item label exactly. Command categories in the palette (Changes, Editing, Navigation, Project, Workspace) use: Änderungen, Bearbeitung, Navigation, Projekt, Arbeitsbereich.

## 3. Term table

| English | German | Note |
|---|---|---|
| project | Projekt | |
| file | Datei | |
| folder | Ordner | |
| main file | Hauptdatei | |
| new file | Neue Datei | |
| new folder | Neuer Ordner | |
| document | Dokument | |
| preview | Vorschau | |
| compile (verb) | kompilieren | LaTeX and Typst users say "kompilieren". Button: "Kompilieren" |
| compiling | Kompiliere… / Wird kompiliert | Use "Wird kompiliert…" in status lines |
| compile now | Jetzt kompilieren | |
| compiler | Compiler | Kept in English, as in LaTeX documentation |
| render (verb) | rendern | Used for Markdown, HTML, Quarto (no compile step). For LaTeX and Typst use "kompilieren" |
| render (noun), rendering | Darstellung | "Rendering" is acceptable in technical notes |
| engine | Engine | Kept in English |
| source | Quelltext | For "the source". "Quelle" is only for citation sources |
| source file | Quelldatei | |
| preamble | Präambel | Standard LaTeX term |
| package (LaTeX) | Paket | "LaTeX-Paket" when ambiguous |
| bibliography | Literaturverzeichnis | "Bibliografie" only for the BibTeX file/topic. Insert menu: "Literaturverzeichnis" |
| bibliography file | Literaturdatei | |
| citation | Zitat | "Zitation" is not used |
| cite (verb) | zitieren | |
| cross-reference | Querverweis | |
| reference | Verweis | |
| theorem | Satz | Mathematical sense. Environment name "Theorem" is also seen; use "Satz" |
| proof | Beweis | |
| lemma | Lemma | |
| definition | Definition | |
| figure | Abbildung | Float and its numbered label. "Bild" for an image file |
| image | Bild | |
| table | Tabelle | |
| equation | Gleichung | |
| formula | Formel | |
| math | Formel | Menu and command text "Formel einfügen"; "Mathematik" only for the category |
| symbol | Symbol | |
| caption | Beschriftung | |
| label (LaTeX \label) | Label | Kept; "Marke" is rarely used |
| footnote | Fußnote | |
| heading | Überschrift | |
| section | Abschnitt | |
| outline | Gliederung | Word and LaTeX tools use "Gliederung" |
| word count | Wortanzahl | Short form "Wörter" in tight tables |
| character | Zeichen | |
| page | Seite | |
| template | Vorlage | |
| save as template | Als Vorlage speichern | |
| track changes | Änderungen nachverfolgen | Feature name "Änderungsverfolgung" |
| change (an edit) | Änderung | |
| suggestion | Vorschlag | |
| suggest (verb) | vorschlagen | |
| accept | Annehmen | Word uses "Annehmen"; "Akzeptieren" is not used |
| reject | Ablehnen | |
| accept all / reject all | Alle annehmen / Alle ablehnen | |
| comment (noun) | Kommentar | |
| comment (verb) | Kommentieren | |
| resolve | Erledigen | Word uses "Erledigt"; state: "Erledigt" |
| resolved | Erledigt | |
| reopen | Wieder öffnen | |
| reply | Antworten | Noun: "Antwort" |
| mention | Erwähnung | |
| highlight | Hervorhebung | Verb: "Hervorheben" |
| share | Teilen | |
| share link | Freigabelink | Compound form shorter than "Link zum Teilen" |
| link | Link | Kept; "Verknüpfung" is wrong here |
| access | Zugriff | |
| permission | Berechtigung | |
| owner | Eigentümer | |
| editor (role) | Bearbeiter | The text editor is "Editor" |
| commenter | Kommentator | Short and used in Google Docs as "Kommentator" |
| reader | Leser | |
| role | Rolle | |
| collaborator | Mitarbeiter | |
| collaboration | Zusammenarbeit | |
| history | Verlauf | Matches Google Docs "Versionsverlauf"; short form "Verlauf" |
| version | Version | |
| bookmark | Lesezeichen | |
| draft | Entwurf | |
| restore | Wiederherstellen | |
| recover | Wiederherstellen | Same word; note the context |
| compare | Vergleichen | |
| comparison base | Vergleichsbasis | |
| diff | Unterschiede | |
| branch | Branch | Kept in English, as in Git documentation |
| commit (noun, verb) | Commit | Kept; verb "committen" |
| push | Push | Kept; verb "pushen" |
| pull | Pull | Kept; verb "pullen" |
| repository | Repository | Kept; "Git-Repository" |
| remote | Remote | Kept |
| clone | Klonen | |
| fork | Fork | Kept in the Git sense; see ambiguous labels |
| merge | Zusammenführen | |
| conflict | Konflikt | |
| sync | Synchronisieren | Noun: "Synchronisierung" |
| synced | Synchronisiert | |
| connect / disconnect | Verbinden / Trennen | |
| sign in | Anmelden | |
| sign out | Abmelden | |
| account | Konto | |
| erase account | Konto löschen | |
| username / handle | Benutzername | |
| password | Passwort | |
| server | Server | |
| settings | Einstellungen | |
| preferences | Einstellungen | Same word |
| theme | Design | "Theme" is not used |
| dark / light | Dunkel / Hell | |
| appearance | Darstellung | |
| language | Sprache | |
| font | Schriftart | |
| font size | Schriftgröße | |
| line height | Zeilenhöhe | |
| colour | Farbe | |
| upload | Hochladen | Noun "Upload" |
| download | Herunterladen | Noun "Download"; button: "Herunterladen" |
| export | Exportieren | |
| import | Importieren | |
| trash | Papierkorb | |
| move to trash | In den Papierkorb legen | Short: "In Papierkorb" |
| delete | Löschen | |
| delete for good | Endgültig löschen | |
| rename | Umbenennen | |
| duplicate | Duplizieren | |
| undo / redo | Rückgängig / Wiederholen | Microsoft: "Rückgängig", "Wiederholen" |
| cut / copy / paste | Ausschneiden / Kopieren / Einfügen | |
| select all | Alles auswählen | |
| search | Suchen | |
| find | Suchen | |
| replace | Ersetzen | |
| save | Speichern | |
| saved | Gespeichert | |
| saving | Wird gespeichert… | |
| cancel | Abbrechen | |
| tag | Tag | Kept in English, common in German software |
| favorites | Favoriten | |
| recent | Zuletzt geöffnet | Short: "Zuletzt" |
| shared with me | Mit mir geteilt | |
| token | Token | Kept |
| quota | Kontingent | "Quota" is rare; use "Speicherplatz" for storage amounts |
| storage | Speicher | |
| used | belegt | "5 MB von 50 MB belegt" |
| backup | Backup | Kept; plural "Backups" |
| agent | Agent | |
| AI agent | KI-Agent | |
| assistant | Assistent | |
| chat | Chat | |
| conversation | Unterhaltung | |
| message | Nachricht | |
| send | Senden | |
| Run code | Code ausführen | Switch label. Stays identical everywhere |
| run | Ausführen | |
| desktop app | Desktop-App | |
| this computer | Dieser Computer | |
| this browser | Dieser Browser | |
| this device | Dieses Gerät | |
| offline | Offline | |
| online | Online | |
| available offline | Offline verfügbar | |
| diagnostics | Diagnose | |
| error | Fehler | |
| warning | Warnung | |
| build log | Build-Protokoll | |
| spelling | Rechtschreibung | |
| grammar | Grammatik | |
| proofing | Korrektur | |
| editor (text editor) | Editor | |
| key bindings | Tastenbelegung | |
| shortcut | Tastenkürzel | Microsoft uses "Tastenkombination"; use "Tastenkürzel" for the feature |
| command palette | Befehlspalette | |
| command | Befehl | |
| panel | Bereich | "Panel" is not used |
| sidebar | Seitenleiste | |
| tab | Tab | |
| dialog | Dialog | |
| layout | Layout | |
| zoom | Zoom | |
| fullscreen | Vollbild | |
| lock (preview) | Fixieren | See ambiguous labels |
| format (document format) | Format | |
| autocomplete | Autovervollständigung | |
| visual markup | Visuelle Auszeichnung | |
| update | Aktualisieren | |
| version number | Versionsnummer | |
| size | Größe | |
| name | Name | |
| date | Datum | |
| path | Pfad | |
| folder (Git) | Ordner | |
| try demo | Ausprobieren | "Jetzt ausprobieren" for the hero button |

## 4. Ambiguous short labels

| English | Sense in this app | German |
|---|---|---|
| Open | Open a file or project | Öffnen |
| Open | Adjective (a thread is open) | Offen |
| Run | Execute code | Ausführen |
| Run | Run a task (agent) | Starten |
| Fork | Make an independent copy of a document at a version | Kopie erstellen (verb phrase); noun "Fork" only in Git context |
| Split | Split the view into two panes | "Teilen" is reserved for share. Use "Ansicht teilen" or "Geteilte Ansicht" |
| Draft | Unsent text or an unpublished version | Entwurf |
| Label | LaTeX \label, version label | Label (LaTeX); "Bezeichnung" for a name given to a version |
| Caption | Text under a figure | Beschriftung |
| Source | The editable text | Quelltext |
| Source | Origin of data | Quelle |
| Preview | Rendered view | Vorschau |
| Render | Produce output | Rendern; for LaTeX and Typst "Kompilieren" |
| Find | Search within the document | Suchen |
| Replace | Substitute text | Ersetzen |
| Replace | Overwrite a file | Ersetzen |
| Format | Text formatting (verb) | Formatieren |
| Format | File format (noun) | Format |
| Insert | Add content at the cursor | Einfügen |
| Close | Close a dialog or file | Schließen |
| Clear | Empty a field or list | Leeren |
| Clear | Remove a selection | Auswahl aufheben |
| Reset | Return a setting to default | Zurücksetzen |
| Revoke | Cancel a share link or token | Widerrufen |
| Star | Mark a favorite | Favorisieren; state "Favorit" |
| Pin | Keep an item at the top | Anheften |
| Lock | Fix the preview on a file | Fixieren |
| Lock | Block editing | Sperren |
| Unlock | Release a fixed preview | Fixierung aufheben |
| Unlock | Release a lock | Entsperren |
| Back | Go to the previous screen | Zurück |
| Done | Finish an action | Fertig |
| Apply | Make a choice take effect | Anwenden |
| Keep | Keep a version in a conflict | Beibehalten |
| Keep | Leave a file in place | Behalten |
| Discard | Throw away changes | Verwerfen |
| Move | Move a file | Verschieben |
| Copy | Duplicate to the clipboard | Kopieren |
| Copy link | Put a link on the clipboard | Link kopieren |
| Link | Hyperlink | Link |
| Note | A hint or remark | Hinweis |
| Note | A user note in the document | Notiz |
| Status | State of a process | Status |
| Default | Preselected value | Standard |
| Custom | User defined | Benutzerdefiniert |
| System | Follow the operating system | System |
| Automatic | Chosen by the app | Automatisch |
| Cancel | Abort | Abbrechen |
| Retry | Try again | Erneut versuchen |
| Try again | Try again | Erneut versuchen |
| Add | Add an item | Hinzufügen |
| Remove | Take away from a list | Entfernen |
| Delete | Destroy | Löschen |
| Select | Choose | Auswählen |
| Show / Hide | Visibility | Anzeigen / Ausblenden |
| Edit | Change content | Bearbeiten |
| Edit (role) | Permission level | Bearbeiten |
| Read (role) | Permission level | Lesen |
| Comment (role) | Permission level | Kommentieren |
| Sent | Delivered | Gesendet |
| New | Create a new item | Neu |

## 5. Kept in English

- Product and format names: LibrePaper, LaTeX, Typst, Quarto, Markdown, HTML, PDF, DOCX, Word, Zotero, GitHub, Git, Harper, Pandoc, BibTeX, Biber, TeX Live, Vim, Emacs, Claude Code, MCP, ACP, Tauri.
- Keyboard key names are translated as on German keyboards only where established: Strg (Ctrl), Umschalt (Shift), Alt, Esc, Enter, Tab, Leertaste (Space), Rück (Backspace), Entf (Delete). On macOS keep the symbols ⌘, ⌥, ⇧. Write "Strg+S" with no spaces around "+".
- Git terms used by developers: Branch, Commit, Push, Pull, Repository, Remote, Fork.
- Other loanwords that stay: Token, Tag, Chat, Backup, Desktop-App, Compiler, Engine, Link, Offline, Online, Browser, Server, Button (avoid; use "Schaltfläche" only in help text).
- Code and file names, extensions, command names, and placeholders in braces are never translated.

## 6. Units and numbers

- Byte units: B, KB, MB, GB, TB (decimal use as in the English source). Binary units KiB, MiB, GiB are kept as written. Put a no-break space between number and unit: "12 MB".
- Decimal separator: comma ("1,5 GB"). Thousands separator: no-break space or period ("12.345"); rely on locale formatting where the code provides it.
- Time: "vor {minutes} Min.", "vor {hours} Std.", "vor {days} Tagen". "Today" is Heute, "Yesterday" is Gestern.
- Dates: DD.MM.YYYY in hand-written examples; rely on locale formatting otherwise.
- Counts: "{count} Fehler" (singular and plural are the same), "{count} Version" / "{count} Versionen", "{count} Datei" / "{count} Dateien", "{count} Projekt" / "{count} Projekte". Use "ein" forms only if the source has a separate "one" branch.
- Percent: "50 %" with a no-break space before the sign.
- Ranges and "of": "{done} von {total}", "{used} von {quota} belegt".
