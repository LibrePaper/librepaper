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
content delivery network. A document published that way would tell that network
who is reading it, on every open, whoever hosts LibrePaper.

LibrePaper renders Quarto with every resource embedded, so documents built
through the companion do not have this dependency and make no network request
at all. A document rendered somewhere else and uploaded can still carry one.
Publishing reports the hosts it finds, and the fix is to re-render with
`embed-resources: true`.

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

## The companion is not looked for until you ask

The page can reach the LibrePaper app on your own computer, over loopback, to
build a document with the tools installed there. Asking is not free: a request
to loopback is what makes a browser put up its local-network permission —
Firefox's wording is that the site wants access to other apps and services —
and a prompt like that, arriving at the moment a document opens, reads as an
accusation.

So the page never looks on its own. Opening a document reaches nothing, even
one this browser has already paired with the app. The first loopback request
is made by a gesture that needs it: turning on local execution, choosing a
local build tool, a Zotero lookup, or opening the Local app settings — the one
pane that exists to show the companion. The Build settings pane does not look
when it opens: most of what it offers is a choice between engines in this
browser, and asking which tools are installed on your computer is not the price
of choosing pdfLaTeX. Its local tools are offered rather than greyed out until
somebody picks one, and picking one is the question. Refusing the permission
costs only those; everything the browser renders by itself is unaffected.
