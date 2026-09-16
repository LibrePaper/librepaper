# What a LibrePaper deployment can see, and what a document can see

This page is for operators and for anyone deciding whether to read a paper
through a link somebody sent them. It describes what actually happens, not
what would be ideal.

## The operator sees everything

There is no end-to-end encryption and none is planned. Whoever runs the server
and its database can read every draft, every comment, every identity and every
presence event in the clear. Backups are written unencrypted today, and a
backup archive is the whole deployment in plain form. If you are hosting for
other people, say so in your own words somewhere they will read it.

## A document can tell its author who read it

A published document is a web page, and it runs its own code. LibrePaper serves
it from a separate hostname so it cannot reach the reader's session, their
comments, or anything in the application around it. That boundary holds.

What it can still do is fetch things from the open web. An image, a web font or
a live data request goes to whatever host the document names, and that host
learns the reader's address, their rough location, their browser and the exact
moment they opened the paper, along with how often they came back. This needs
no JavaScript at all; an image tag is enough.

**For anonymous or blind review, this means a document can identify its
reviewer, and the person best placed to arrange that is the author.** LibrePaper
does not prevent it. If anonymity matters for your process, the workable
answers are to distribute a PDF, or to require that documents are published
with every resource embedded, which is what `embed-resources` does for Quarto
and what LibrePaper's own pandoc path already does.

What a document may **not** do is fetch code from another host. Its own inline
and embedded scripts run normally; a `<script>` tag pointing at another server
does not load. That keeps a document's behavior fixed at the moment it was
published, so what a reviewer read is what runs later, and it keeps a
compromise of some third-party host out of every reader's browser.

## Documents rendered elsewhere may already be reaching out

Quarto's default HTML output loads its mathematics renderer from a public
content delivery network. A document published that way tells that network who
is reading it, on every open, whoever hosts LibrePaper. Rendering with
`embed-resources: true` removes the dependency by inlining everything.

## Reading is not invisible

An anonymous reader gets a persistent signed visitor credential so their
comments hold together across visits. While a document is open, the
collaboration layer broadcasts presence and cursor position, so an author
watching the document sees a reviewer arrive, sees roughly where they are in
the text, and sees when they leave. There is currently no way to read without
being visible this way.

## What a pseudonym does and does not hide

A comment can carry a display pseudonym instead of an account name. It hides
who you are from other readers. It does not hide you from the operator: the
account id travels with the write and is stored beside the comment. Treat a
pseudonymous comment as pseudonymous to the room and attributable to whoever
runs the server.

## The LaTeX mirror

Browsers compiling LaTeX in the page fetch TeX packages from a public mirror,
which by default is a service the LibrePaper project runs. That service sees
each reader's address and the exact set of packages a document pulls, which is
a usable fingerprint of the document. Bytes are verified against a digest, so
this is a privacy and availability dependency rather than a way to tamper with
a build. Operators who do not want it can host a mirror.
