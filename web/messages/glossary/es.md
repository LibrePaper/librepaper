# Spanish glossary (es)

## 1. Register and conventions

- Language: neutral international Spanish. Avoid regionalisms and voseo. Do not use "vosotros" and do not use "ordenador" or "computadora" (use "equipo").
- Address: impersonal forms and the infinitive where possible. When the user must be addressed directly, use "usted" (never "tú"), as Microsoft and Google do in mainstream software. Example: "Escriba su nombre de usuario", "Su cuenta se ha borrado". Prefer impersonal phrasing: "No se pudo guardar el archivo".
- Buttons and menu items: infinitive ("Guardar", "Abrir", "Compartir"). Titles and group labels are nouns ("Configuración", "Historial").
- Instructions in sentences: imperative in the "usted" form ("Seleccione un archivo"), or impersonal ("Se necesita iniciar sesión").
- Capitalization: sentence case. Only the first word and proper nouns are capitalized. Months and weekdays are lowercase.
- Quotation marks: angular quotes «like this» for primary quotes, with no inner spaces. English curly quotes in source strings become « » in the translation. Keep a straight " only inside code.
- Punctuation: opening marks are required: "¿Desea continuar?" and "¡Listo!". No space before ":" ";" "?" "!". Use a single ellipsis character "…" with no space before it, as in the English source.
- No em dashes or en dashes. Use commas, colons or parentheses.
- Decimal separator: comma in Spain, point in much of Latin America. Do not format numbers in strings; leave formatting to the locale-aware code. Use "{count}" as given.
- Lists in a sentence: "a, b y c" with no serial comma before "y".
- Gender and number: use the masculine plural for mixed groups, or neutral wording ("las personas que colaboran") when it stays short. Role nouns such as "propietario", "editor" and "lector" are used as is.
- Plurals: keep the same plural structure as the English message. Spanish has "one" and "other" (and "many" for large round numbers, which can be treated as "other").
- Paper and document terms: use the community terms in the table (preámbulo, referencia cruzada, control de cambios).
- Latin words and Anglicisms: avoid unnecessary ones. Keep only those listed in section 5.

## 2. Menu titles

| English | Spanish | Note |
|---|---|---|
| File | Archivo | |
| Edit | Edición | Use "Edición", the noun, as Microsoft Office does. |
| Insert | Insertar | Menu title. |
| View | Ver | Menu title. Microsoft uses "Vista"; use "Ver" for the menu and "Vista" for a named view. |
| Settings | Configuración | |
| Help | Ayuda | |
| Format | Formato | |
| Tools | Herramientas | |
| Window | Ventana | |

Menu paths in sentences use ">" with spaces and each title as written above. Example: "Ver > PDF", "Archivo > Descargar", "Insertar > Cita". Item names in a path must match the item's own label exactly.

## 3. Term table

| English | Spanish | Note |
|---|---|---|
| project | proyecto |  |
| file | archivo | Not "fichero" (Spain-only leaning). |
| folder | carpeta |  |
| main file | archivo principal |  |
| document | documento |  |
| preview | vista previa | Noun. Not "previsualización". |
| render (verb) | compilar | For LaTeX/Typst/Quarto output. "Render" the command is "Compilar". |
| render (noun), rendering | resultado compilado | Use "renderizado" only for HTML/browser rendering. |
| compile | compilar |  |
| compile error | error de compilación |  |
| diagnostics | diagnósticos |  |
| source | código fuente | Short form "fuente" is fine in tight labels. Never for typeface. |
| source mode | modo código fuente |  |
| editor (the text editor) | editor |  |
| preamble | preámbulo | LaTeX term. |
| package (LaTeX) | paquete |  |
| document class | clase de documento |  |
| environment (LaTeX) | entorno |  |
| bibliography | bibliografía |  |
| citation | cita |  |
| cite (verb) | citar |  |
| cross-reference | referencia cruzada |  |
| reference | referencia |  |
| theorem | teorema |  |
| lemma | lema |  |
| proof | demostración |  |
| figure | figura |  |
| table | tabla |  |
| equation | ecuación |  |
| math | matemáticas | Label of math input: "Matemáticas". |
| symbol | símbolo |  |
| caption | pie de figura | Generic caption: "leyenda" if not under a figure; keep "pie de figura" for figures and tables. |
| label | etiqueta | LaTeX \label and general labels. Tags use "etiqueta" too, see ambiguity table. |
| outline | esquema | Document structure panel. Not "contorno". |
| heading | encabezado | Section heading. Page header is "encabezado de página". |
| section | sección |  |
| footnote | nota al pie |  |
| word count | recuento de palabras |  |
| character count | recuento de caracteres |  |
| track changes | control de cambios | Word's Spanish term. |
| suggestion | sugerencia |  |
| change (an edit) | cambio |  |
| accept | aceptar |  |
| reject | rechazar |  |
| accept all | aceptar todo |  |
| comment (noun) | comentario |  |
| comment (verb) | comentar |  |
| resolve | resolver |  |
| reopen | reabrir |  |
| reply | responder | Verb. Noun: "respuesta". |
| share | compartir |  |
| share link | enlace para compartir |  |
| link | enlace |  |
| owner | propietario | Use "propietario" for both genders in UI. |
| editor (role) | editor |  |
| commenter | comentarista | Role. Not "comentador". |
| reader | lector |  |
| viewer | lector | Same as reader if it appears. |
| collaborator | colaborador |  |
| permission | permiso |  |
| access | acceso |  |
| history | historial |  |
| version | versión |  |
| bookmark | marcador | Named saved version. Not "favorito". |
| compare | comparar |  |
| restore | restaurar |  |
| branch | rama |  |
| commit | commit | Kept in English, as in most Spanish developer writing. Verb: "hacer commit". |
| push | push | Kept in English. Verb: "hacer push". |
| pull | pull | Kept in English. Verb: "hacer pull". |
| repository | repositorio |  |
| clone | clonar |  |
| merge | fusionar |  |
| conflict | conflicto |  |
| sync | sincronizar | Noun: "sincronización". |
| upload | subir | Mainstream (Google, Microsoft). Noun: "subida". |
| download | descargar |  |
| export | exportar |  |
| import | importar |  |
| trash | papelera |  |
| delete | eliminar | Prefer "eliminar" for destructive actions. |
| erase (account) | borrar | Account erasure: "Borrar cuenta". |
| rename | renombrar |  |
| duplicate | duplicar |  |
| create | crear |  |
| new | nuevo | Agrees with noun: "Nuevo proyecto", "Nueva carpeta". |
| template | plantilla |  |
| tag | etiqueta | Project tag. See ambiguity table for Label vs Tag. |
| favorites | favoritos |  |
| token | token | Kept in English, plural "tokens". |
| quota | cuota |  |
| storage | almacenamiento |  |
| backup | copia de seguridad | Plural: "copias de seguridad". |
| agent | agente |  |
| AI agent | agente de IA |  |
| assistant | asistente |  |
| chat | chat |  |
| message | mensaje |  |
| Run code | Ejecutar código | Top-bar switch label. |
| run | ejecutar |  |
| preview panel | panel de vista previa |  |
| desktop app | aplicación de escritorio |  |
| this computer | este equipo | "Equipo" is neutral across regions (Microsoft). Not "ordenador" or "computadora". |
| this browser | este navegador |  |
| browser | navegador |  |
| offline | sin conexión |  |
| online | en línea |  |
| account | cuenta |  |
| sign in | iniciar sesión |  |
| sign out | cerrar sesión |  |
| sign up | registrarse |  |
| handle | nombre de usuario |  |
| name | nombre |  |
| email | correo electrónico |  |
| password | contraseña |  |
| settings | configuración | Microsoft and Google both use it. |
| preferences | preferencias |  |
| theme | tema |  |
| dark | oscuro |  |
| light | claro |  |
| palette | paleta |  |
| font | fuente tipográfica | Say "tipografía" or "fuente" in font settings; disambiguate from source with context. |
| font size | tamaño de fuente |  |
| line height | interlineado |  |
| layout | diseño |  |
| panel | panel |  |
| sidebar | barra lateral |  |
| toolbar | barra de herramientas |  |
| tab | pestaña |  |
| dialog | cuadro de diálogo |  |
| menu | menú |  |
| shortcut | atajo |  |
| key bindings | combinaciones de teclas |  |
| keyboard shortcuts | atajos de teclado |  |
| undo | deshacer |  |
| redo | rehacer |  |
| copy | copiar |  |
| paste | pegar |  |
| cut | cortar |  |
| select all | seleccionar todo |  |
| find | buscar |  |
| replace | reemplazar |  |
| replace all | reemplazar todo |  |
| search | buscar |  |
| case sensitive | distinguir mayúsculas y minúsculas |  |
| regular expression | expresión regular |  |
| whole word | palabra completa |  |
| save | guardar |  |
| saved | guardado |  |
| cancel | cancelar |  |
| confirm | confirmar |  |
| close | cerrar |  |
| open | abrir |  |
| done | listo |  |
| apply | aplicar |  |
| reset | restablecer |  |
| clear | borrar | Clear a field or list. See ambiguity table. |
| revoke | revocar |  |
| retry | reintentar |  |
| loading | cargando |  |
| error | error |  |
| warning | advertencia |  |
| spelling | ortografía |  |
| grammar | gramática |  |
| spellcheck | corrector ortográfico |  |
| dictionary | diccionario |  |
| language | idioma |  |
| image | imagen |  |
| alt text | texto alternativo |  |
| page | página |  |
| line | línea |  |
| column | columna |  |
| row | fila |  |
| list | lista |  |
| bold | negrita |  |
| italic | cursiva |  |
| heading level | nivel de encabezado |  |
| code block | bloque de código |  |
| inline code | código en línea |  |
| Vim mode | modo Vim |  |
| engine | motor | TeX engine and similar. |
| cache | caché |  |
| log | registro | Compile log. |
| free plan | plan gratuito |  |
| public | público |  |
| private | privado |  |
| invite | invitar |  |
| member | miembro |  |
| admin | administrador |  |
| operator | operador | Server operator. |
| server | servidor |  |
| self-hosted | autoalojado |  |
| companion | aplicación complementaria | Only if the string still refers to it. |
| integration | integración |  |
| connection | conexión |  |
| connect | conectar |  |
| disconnect | desconectar |  |
| runner | ejecutor | Agent runner. Say "ejecutor del agente" on first use in a string if unclear. |
| task | tarea |  |
| trim | recortar | Trim history. Describe: "Recortar historial". |

## 4. Ambiguous short labels

| English | Spanish | Sense and note |
|---|---|---|
| Open | Abrir | Verb for files and projects. "Abierto" only as a status. |
| Run | Ejecutar | Run code. "Compilar" is for compile; never use "Ejecutar" for compile. |
| Fork | Bifurcar | Copy someone else's project into your own. Noun: "bifurcación". Git-style fork in code contexts also "bifurcar". |
| Split | Dividir | Split the view or editor: "Dividir vista". Layout mode "Show editor and preview" stays descriptive. |
| Draft | Borrador |  |
| Label | Etiqueta | LaTeX label and version label. Project tags are also "etiqueta"; where both appear together, say "etiqueta de proyecto" for tags. |
| Caption | Pie de figura | Leyenda when not tied to a figure or table. |
| Source | Código fuente | Fuente alone is acceptable in tight space. Never "origen". |
| Preview | Vista previa |  |
| Render | Compilar | HTML preview uses "Renderizar" if the string is about browser rendering. |
| Find | Buscar |  |
| Replace | Reemplazar |  |
| Format | Formato | Noun (file format, text format). Verb "Dar formato". |
| Insert | Insertar |  |
| Close | Cerrar |  |
| Clear | Borrar | Empty a field, filter or list. Use "Quitar" to remove one item from a selection. |
| Reset | Restablecer |  |
| Revoke | Revocar | Tokens and links. |
| Star | Marcar como favorito | Icon label. Noun: "favorito". |
| Pin | Fijar |  |
| Lock | Bloquear |  |
| Unlock | Desbloquear |  |
| Back | Atrás | Navigation. "Volver" for "Back to tasks": "Volver a las tareas". |
| Done | Listo |  |
| Apply | Aplicar |  |
| Keep | Conservar |  |
| Discard | Descartar |  |
| Move | Mover |  |
| Copy | Copiar |  |
| Link | Enlace | Noun. Verb "vincular". |
| Note | Nota |  |
| Status | Estado |  |
| Default | Predeterminado | Agrees in gender: "Predeterminada" for feminine nouns. |
| Custom | Personalizado |  |
| System | Sistema | Theme option: "Sistema". |
| Automatic | Automático | Agrees in gender. |
| Bookmark | Marcador |  |
| Tag | Etiqueta |  |
| Branch | Rama |  |
| Account | Cuenta |  |
| Home | Inicio |  |

## 5. Kept in English

- LibrePaper, LaTeX, Typst, Quarto, Markdown, HTML, PDF, DOCX, Word, Zotero, GitHub, Git, Harper, Pandoc, BibTeX, Biber, TeX Live, Vim, Emacs, Claude Code, Claude, Codex.
- Key names: Ctrl, Alt, Shift, Enter, Esc, Tab, Cmd. Write them as on the keyboard: "Ctrl+S". Keep Cmd and the symbols on macOS.
- Git words commonly kept: commit, push, pull.
- Also kept: token, chat, URL, ID, API, MCP, ACP, CSS, DOI, ORCID, OIDC, Wi-Fi.
- Placeholders such as {name} and {count}, file extensions, command-line text, shell commands, and code snippets are never translated.
- "Run code" stays a UI phrase in Spanish ("Ejecutar código"); the underlying flag names are not translated.

## 6. Units and numbers

- Byte units: B, KB, MB, GB, TB, as in the English source. Do not translate them to "o" or "Kb". Put a no-break space between the number and the unit: "50 MB".
- Counts: use the count placeholder with the plural form. "1 palabra", "{count} palabras".
- Percent sign: no-break space before "%" is correct in Spanish (RAE) but mainstream software often omits it; follow the English: write "50 %" with a no-break space.
- Dates and times: leave to locale-aware formatting. In text, use the 24-hour clock where the source does.
- File sizes and counts read as numerals, not words.
