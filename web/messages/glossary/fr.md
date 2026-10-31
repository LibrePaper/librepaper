# French glossary (fr)

The target is Canadian French (Quebec) that also reads naturally to readers in France. Where the two differ, follow the OQLF and the mainstream Microsoft, Apple and Google localization style in French. The French catalogue is the reference for choices already made. This file codifies them.

## 1. Register and conventions

- Address: "vous", always, never "tu". Prefer a neutral sentence when the user does not need to be addressed ("Le projet a été enregistré.").
- Buttons and menu items: infinitive ("Enregistrer", "Partager", "Ouvrir un fichier"). Never the imperative on a button. Menu titles are nouns (see section 2). Command-palette entries follow the same infinitive rule.
- Messages in sentences use the imperative only when they instruct the reader ("Ouvrez un fichier pour utiliser cette tâche.").
- Error messages: say what failed. The dominant form in the catalogue is passive: "Le fichier n’a pas pu être enregistré." Use "Impossible de ..." only when there is no clear subject ("Impossible de créer le lien."). Do not use "Échec de".
- Status lines: a bare noun plus an ellipsis, as in "Chargement…", "Enregistrement…", "Connexion…". Add "en cours" only when the English says "in progress" or when the status is a sentence ("Compilation en cours").
- Field labels: no article. "Nom du projet", "Nombre de colonnes", "Niveau".
- Apostrophe: use the typographic ’ (U+2019), as the catalogue does ("l’agent", "d’accès").
- Quotation marks: guillemets « » with a no-break space (U+00A0) inside each: « texte ». Nested quotes use “ ”. Never straight quotes in translated prose. A placeholder in quotes in the English source becomes « {query} ».
- Colon: a no-break space (U+00A0) before ":" only. No space before ";", "!" or "?" (OQLF rule). Example: "Nom :" with a no-break space between "Nom" and the colon.
- Ellipsis: the single character … (U+2026) with no space before it, exactly where English has it ("Choisir un fichier…").
- Capitalization: sentence style. Only the first word and proper nouns are capitalized ("Nouveau projet", not "Nouveau Projet"). Day and month names are lower case. Language names are lower case.
- Numbers: a no-break space between a number and its unit ("12 Mo", "5 min") and before "%" ("50 %"). Decimal comma ("1,5 Go"). Thousands separator is a no-break space ("12 345"). Rely on locale formatting where the code formats numbers.
- Dashes: do not use em or en dashes. Use commas, colons or parentheses.
- Gender and inclusion: use neutral wording first ("personnes", "compte", "membres"). Where a role noun is unavoidable, use the generic masculine as mainstream software does (propriétaire, éditeur, lecteur, commentateur). Do not use middle dots, slashes or parenthetical feminine forms.
- Length: French runs 15 to 30 percent longer than English. Prefer the shorter correct term for buttons and tab labels. Do not abbreviate unless the English is already an abbreviation or the key says "short" (see "Saved" below).
- Placeholders such as {count}, {path}, {who} stay unchanged and in a position that reads correctly. French plural rule: 0 and 1 are singular, so "0 fichier", "1 fichier", "2 fichiers".
- Anglicisms: avoid "e-mail", "upload", "chat", "link" in prose. Use the French terms in the tables. Keep an English word only where section 5 says so.

## 2. Menu titles

| English | French | Note |
|---|---|---|
| File | Fichier | |
| Edit | Édition | Noun, as in Microsoft and Apple French. The verb "Modifier" is for buttons and roles |
| Insert | Insertion | Noun for the menu. The button is "Insérer" |
| View | Affichage | Noun for the menu. The verb "Afficher" is for buttons |
| Settings | Paramètres | Never "Réglages" or "Préférences" |
| Help | Aide | |
| Tools | Outils | |
| Format | Format | Menu title and noun. The verb "Formater" is for buttons |
| Download | Télécharger | Menu of download formats keeps the infinitive, as in the catalogue |
| Command palette | Palette de commandes | |

Menu paths in sentences use the same words with " > " (a space on each side): "Affichage > PDF", "Fichier > Enregistrer comme modèle", "Paramètres > Sauvegardes". The item after the arrow must match its label exactly.

Command categories in the palette: Changes = Modifications, Editing = Édition, Navigation = Navigation, Project = Projet, Workspace = Espace de travail.

## 3. Term table

### Projects and files

| English | French | Note |
|---|---|---|
| project | projet | |
| new project | Nouveau projet | |
| file | fichier | |
| folder | dossier | |
| main file | fichier principal | |
| new file | Nouveau fichier | |
| new folder | Nouveau dossier | |
| untitled | Sans titre | |
| rename | renommer | |
| duplicate | dupliquer | Copy of a file or project |
| move | déplacer | |
| delete | supprimer | Default destructive verb. See the drift decisions for "Retirer" |
| trash | corbeille | |
| move to trash | Mettre à la corbeille | One form everywhere |
| restore | restaurer | |
| empty trash | Vider la corbeille | |
| upload | téléverser | Noun: téléversement |
| download | télécharger | Noun: téléchargement |
| import | importer | |
| export | exporter | |
| template | modèle | |
| tag | étiquette | Project tags. Never "balise" (that is an HTML tag) |
| favorites | favoris | |
| archive | archive | A stored copy of a version or project |
| asset | ressource | Uploaded fonts, images and similar files |
| conflict | conflit | |
| merge | fusionner | Noun: fusion |

### Documents and typesetting

| English | French | Note |
|---|---|---|
| source | source | The editable text. Also "texte source" in a sentence |
| source file | fichier source | |
| preview | aperçu | |
| compile (verb) | compiler | For LaTeX and Typst. Noun: compilation. Participle: compilé |
| compiler (the tool) | compilateur | |
| engine | moteur | TeX engine, render engine |
| render (verb) | produire le rendu | Short button: "Compiler" for LaTeX and Typst, "Générer" is not used |
| render, rendering (noun) | rendu | |
| preamble | préambule | |
| package (LaTeX) | paquet | "paquetage" is not used in new strings. Typst packages are also "paquets" |
| bibliography | bibliographie | |
| bibliography file | fichier de bibliographie | |
| citation | citation | |
| cite (verb) | citer | |
| reference (bibliography entry) | référence | |
| cross-reference | renvoi croisé | Short form "renvoi" |
| theorem | théorème | |
| proof | démonstration | "Preuve" is not used |
| figure | figure | |
| table | tableau | |
| equation | équation | |
| formula | formule | |
| inline math | formule en ligne | |
| display math | formule hors texte | |
| math | mathématiques | Short palette label: "Maths" is not used |
| symbol | symbole | |
| caption | légende | |
| label (LaTeX \label, anchor) | étiquette | See section 4 for the version label sense |
| footnote | note de bas de page | |
| heading | titre | |
| abstract | résumé | |
| appendix | annexe | |
| author | auteur | |
| table of contents | table des matières | |
| outline | plan | The sidebar panel |
| word count | nombre de mots | |
| index (LaTeX index) | index | |
| subscript | indice | |
| superscript | exposant | |
| bold | gras | |
| italic | italique | |
| underline | souligné | |
| strikethrough | barré | |
| highlight (noun) | surlignage | |
| highlight (verb) | surligner | |
| bulleted list | liste à puces | |
| numbered list | liste numérotée | |
| quote, block quote | citation en bloc | "Citation" alone would clash with bibliographic citation |
| code block | bloc de code | |
| link (hyperlink) | lien | |
| layout | mise en page | |
| font | police | |
| font size | taille de police | |
| line height | interligne | |
| zoom | zoom | |
| full screen | plein écran | |
| diagnostics | diagnostics | |
| log | journal | |
| error / warning | erreur / avertissement | |
| spell check | vérification orthographique | |
| spelling / grammar | orthographe / grammaire | |
| dictionary | dictionnaire | |
| autocomplete, completion | complétion | "Saisie semi-automatique" for a setting label |
| language (human) | langue | |
| language (code) | langage | |
| key bindings | raccourcis clavier | |
| shortcut | raccourci | |

### Review and collaboration

| English | French | Note |
|---|---|---|
| comment (noun) | commentaire | |
| comment (verb, role) | commenter | |
| reply | répondre | Noun: réponse |
| resolve | résoudre | Participle: résolu |
| reopen | rouvrir | |
| suggestion | suggestion | |
| suggest | suggérer | |
| track changes | suivi des modifications | |
| change (an edit) | modification | |
| accept | accepter | |
| reject | refuser | Never "rejeter" |
| accept all / reject all | Tout accepter / Tout refuser | |
| mention | mention | |
| share | partager | |
| share link | lien de partage | One term, not "lien d’accès" except for an agent link (see below) |
| access link (agent) | lien d’accès | For an agent, as in the catalogue |
| permission | autorisation | "Droits d’accès" in a longer phrase |
| owner | propriétaire | |
| editor (role) | éditeur | |
| commenter (role) | commentateur | |
| reader (role) | lecteur | |
| collaborator | collaborateur | |
| people | personnes | |
| invite | inviter | |
| revoke | révoquer | Links and tokens |
| expires | Expiration | |
| public / private | public / privé | |
| history | historique | |
| version | version | |
| bookmark | signet | A named version |
| restore this version | Restaurer cette version | |
| compare | comparer | |
| diff | différences | |

### Agent, chat and automation

| English | French | Note |
|---|---|---|
| AI agent | agent IA | Translate the English literally: "Agent" is "agent", "AI agent" is "agent IA" |
| agent | agent | |
| assistant | assistant | The task helper in the sidebar |
| task (assistant) | tâche | |
| chat | clavardage | Noun and the panel name. Verb: clavarder |
| prompt | invite | The text the user writes to the agent |
| Run code | Exécuter le code | Fixed. The top-bar switch |
| run (code) | exécuter | |
| execution | exécution | |
| stop | Arrêter | |
| token | jeton | |
| agent token | jeton d’agent | |
| quota | quota | |
| storage | stockage | |
| usage | utilisation | |
| connect | connecter | |
| connection | connexion | |
| remote connection | connexion à distance | |
| companion (app) | application compagnon | |
| integration | intégration | |
| extension | extension | |

### Git and synchronization

| English | French | Note |
|---|---|---|
| repository | dépôt | |
| branch | branche | |
| commit (noun) | commit | Kept in English, masculine. Verb: "créer un commit" |
| push | envoyer | In a button or sentence: "Envoyer les modifications du projet". The word "push" appears only when naming the Git command |
| pull | obtenir | "Obtenir les modifications du dépôt" |
| fetch | récupérer | |
| sync | synchroniser | Noun: synchronisation. Participle: synchronisé |
| switch branch | changer de branche | |
| remote (Git) | dépôt distant | |
| clone | cloner | |
| fork (Git) | fork | Kept in English in Git context only |
| pending | en attente | |
| backups | sauvegardes | Noun: sauvegarde |
| back up | sauvegarder | |

### Account, settings and app

| English | French | Note |
|---|---|---|
| sign in | se connecter | Button: "Se connecter" |
| sign out | se déconnecter | |
| account | compte | |
| handle (username) | identifiant | |
| settings | paramètres | |
| theme | thème | |
| dark / light | sombre / clair | |
| system (theme) | système | |
| email | courriel | Never "e-mail" or "email" |
| password | mot de passe | |
| privacy | confidentialité | |
| terms | conditions d’utilisation | |
| desktop app | application de bureau | |
| this computer | cet ordinateur | |
| this browser | ce navigateur | |
| browser | navigateur | |
| offline / online | hors ligne / en ligne | |
| window | fenêtre | |
| tab | onglet | |
| panel | panneau | |
| sidebar | barre latérale | |
| toolbar | barre d’outils | |
| dialog | boîte de dialogue | |
| command palette | palette de commandes | |
| try the demo | Essayer | "Essayer maintenant" for the hero button |
| save | enregistrer | Never "sauvegarder" for save. That is for backups |
| undo / redo | annuler / rétablir | |
| copy / paste / cut | copier / coller / couper | |
| search | rechercher | |
| find | rechercher | Same word as search. See section 4 |
| replace | remplacer | |
| select | sélectionner | |
| selection | sélection | |

## 4. Ambiguous short labels

| English | Sense in this app | French |
|---|---|---|
| Open | Open a file or project | Ouvrir |
| Open | Adjective (a thread is open) | Ouvert |
| Run | Execute code | Exécuter |
| Run | Noun, one execution | Exécution |
| Fork | Make an independent copy of a document at a version | Faire une copie |
| Fork | Git term | fork |
| Split | Layout with source and preview side by side | Côte à côte |
| Split | Verb, split the view | Diviser l’affichage |
| Draft | Unsent text or an unpublished version | Brouillon |
| Label | LaTeX label or tag-like name | Étiquette |
| Label | Name given to a saved version | Nom (verb "nommer") |
| Caption | Text under a figure | Légende |
| Source | The editable text | Source |
| Source | Origin of a citation | Source |
| Preview | Rendered view | Aperçu |
| Render | Produce output | Compiler for LaTeX and Typst, "Produire le rendu" for the other formats |
| Find | Search within the document | Rechercher |
| Replace | Substitute text | Remplacer |
| Replace | Overwrite a file | Remplacer |
| Format | Text formatting (verb) | Formater |
| Format | File format (noun) | Format |
| Insert | Add content at the cursor (button) | Insérer |
| Insert | Menu title | Insertion |
| Close | Close a dialog or file | Fermer |
| Clear | Empty a field or list | Effacer |
| Clear | Remove a selection | Désélectionner |
| Reset | Return a setting to default | Réinitialiser |
| Revoke | Cancel a share link or token | Révoquer |
| Star | Mark a favorite | Ajouter aux favoris |
| Pin | Keep an item at the top | Épingler |
| Lock | Fix the preview on a file | Verrouiller |
| Unlock | Release the fixed preview | Déverrouiller |
| Back | Go to the previous screen | Retour |
| Done | Finish an action | Terminé |
| Apply | Make a choice take effect | Appliquer |
| Keep | Keep a version in a conflict | Garder |
| Keep both | Conflict choice | Garder les deux |
| Discard | Throw away changes | Abandonner |
| Move | Move a file | Déplacer |
| Copy | Copy to the clipboard | Copier |
| Copy link | Put a link on the clipboard | Copier le lien |
| Link | Hyperlink | Lien |
| Note | A hint or remark | Remarque |
| Note | A footnote | Note de bas de page |
| Status | State of a process | État |
| Default | Preselected value | Par défaut |
| Custom | User defined | Personnalisé (agree with the noun) |
| System | Follow the operating system | Système |
| Automatic | Chosen by the app | Automatique |
| Cancel | Abort | Annuler |
| Undo | Reverse the last edit | Annuler |
| Retry | Try again | Réessayer |
| Add | Add an item | Ajouter |
| Remove | Take away | Supprimer, see the drift decisions |
| Delete | Destroy | Supprimer |
| Select | Choose | Sélectionner |
| Show / Hide | Visibility | Afficher / Masquer |
| Edit | Verb or role | Modifier |
| Read (role) | Permission level | Lecture |
| Comment (role) | Permission level | Commentaire (as access) or Commenter (as button) |
| Sent | Delivered | Envoyé |
| New | Create a new item | Nouveau (agree with the noun) |
| Dismiss | Close a message | Fermer |
| Dismiss | Decline a hint or suggestion | Ignorer |
| Save | Write to storage | Enregistrer |
| Saved | State | Enregistré |
| Home | Landing page | Accueil |
| Index | LaTeX index | Index |

## 5. Kept in English

- Product and format names: LibrePaper, LaTeX, Typst, Quarto, Markdown, HTML, PDF, DOCX, Word, Zotero, GitHub, Git, Harper, Pandoc, BibTeX, Biber, TeX Live, Vim, Emacs, Claude Code, MCP, ACP, Tauri.
- Keyboard key names: use the French names only where established on French keyboards: Ctrl, Maj (Shift), Alt, Échap (Esc), Entrée (Enter), Tab, Espace, Retour arrière (Backspace), Suppr (Delete). On macOS keep the symbols ⌘, ⌥, ⇧, ⌃.
- Git words: commit, push and pull stay in English only when naming the command or the noun "commit". In buttons use "Envoyer" and "Obtenir". "Fork" stays in Git context only.
- Loanwords that stay: Zoom, commit, and the Vim mode names. Everything else has a French term in section 3.
- Code, file names, extensions, command names and placeholders in braces are never translated.

## 6. Units and numbers

- Byte units use the French octet symbols: o, Ko, Mo, Go, To (the catalogue already uses "Mo"). Binary names KiB, MiB, GiB become Kio, Mio, Gio only if the source shows binary units explicitly. Put a no-break space between number and unit: "12 Mo".
- Decimal separator: comma ("1,5 Go"). Thousands separator: no-break space ("12 345").
- Time ago: « il y a {minutes} min », « il y a {hours} h », « il y a {days} jours ». "Today" is « aujourd’hui », "yesterday" is « hier ».
- Dates: rely on locale formatting. In hand-written examples use "31 octobre 2026".
- Counts: "{count} erreur" / "{count} erreurs", "{count} fichier" / "{count} fichiers", "{count} version" / "{count} versions". Zero is singular.
- Percent: "50 %" with a no-break space before the sign.
- Ranges and "of": « {done} sur {total} », « {used} sur {quota} utilisés ».

## 7. Drift decisions

These resolve the English strings that currently have more than one French translation. Reviewers use the form stated here.

| English | Decision | Note |
|---|---|---|
| Connecting… | Connexion… | Bare noun with ellipsis, like Chargement… and Enregistrement… |
| Remove | Supprimer | Default. Use "Retirer" only when the item keeps existing elsewhere (a person taken off a share list, a file taken out of a selection) |
| Comment | Both right | Noun "Commentaire" for fields and titles. Verb "Commenter" for buttons, roles and access levels |
| Edit | Both right | Menu title "Édition". Button, field and role "Modifier" |
| View | Both right | Menu title "Affichage". Button "Afficher" |
| Agent name | Nom de l’agent | Literal. Use "Nom de l’agent IA" only when the English says "AI agent" |
| Highlight | Both right | Verb button "Surligner". Noun "Surlignage" |
| Comments could not be read ({status}). | Les commentaires n’ont pas pu être lus ({status}). | The passive "n’a pas pu être" form dominates the catalogue |
| Saved | Enregistré | "Enreg." only for the key marked short (history_saved_short) |
| Insert | Both right | Menu title "Insertion". Button "Insérer" |
| Normal | Both right | Agree with the noun: "Normal" (interligne), "Normale" (priorité) |
| Keep both | Garder les deux | "Garder" dominates the catalogue |
| Inline math | Formule en ligne | "Mathématiques en ligne" is not used |
| When the window closes | Lorsque la fenêtre se ferme | "Lorsque" for a conditional title |
| Language | Both right | "Langue" for the interface or a spoken language. "Langage" for a programming language (code block) |
| Dismiss | Both right | "Fermer" closes a message or toast. "Ignorer" declines a hint |
| Columns | Colonnes | A field label reads "Nombre de colonnes", with no article |
| Level | Niveau | No article in a field label |
| Rows | Lignes | "Nombre de lignes" for the field. Never "Rangées" |
| Numbered | Both right | Adjective agrees with the noun ("Numérotée" for an équation). A checkbox with no noun reads "Avec numérotation" |
| The insertion target changed. | L’emplacement d’insertion a changé. | "Point d’insertion" means the cursor and is not used here |
| Move to trash | Mettre à la corbeille | One form everywhere |
| Compiling | Both right | "Compilation en cours" for a status line, "Compilation" for a stage name in a list |
| Split | Côte à côte | The layout name. "Divisé" is not used. Verb: "Diviser l’affichage" |
| Remote connection | Connexion à distance | |
| Agent token | Jeton d’agent | Same wording in titles and aria labels |
| Subscript | Indice | Superscript is "exposant" |
