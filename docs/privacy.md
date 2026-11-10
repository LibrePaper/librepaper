---
title: "Privacy and data retention"
---

> **Warning:** Do not store private or highly sensitive data on librepaper.org.
> Documents are not end-to-end encrypted. The server reads every file so it can
> merge edits from people who write at the same time. The administrators must
> also be able to read files to moderate them. If you need private file
> handling, [host your own server](./host/simple.html).

> **Warning:** librepaper.org erases an account that has not signed in for 365 days. Every document the account owns is deleted with it, including documents shared with other people. No email is sent first. Sign in at least once a year to keep your account, and keep your own copy of work that matters with [local backups](backups.html).

## Who runs librepaper.org

librepaper.org is operated by Vincent Arel-Bundock, in Montréal, Canada. The operator is responsible for the data described on this page. Write to [admin@librepaper.org](mailto:admin@librepaper.org) about anything here.

This page covers librepaper.org only. Other servers that run LibrePaper have their own administrators and their own rules. The [terms of service](terms.html) set the rules for using librepaper.org.

## What your data is used for

librepaper.org uses your data to run the service: to sign you in, to store and typeset your documents, to share them with the people you choose, and to stop abuse. It is used for nothing else.

- There is no advertising, and there are no analytics or tracking scripts.
- Your data is not sold, and it is not shared with anyone for marketing.
- librepaper.org does not use your documents to train AI models. A document that is reported or suspected of breaking the [terms of service](terms.html) may be sent to a large language model for a first assessment, as described under [what leaves librepaper.org](#what-leaves-librepaperorg). A [writing assistant](#what-leaves-librepaperorg) you start yourself sends what it reads to the AI service you chose, under that service's own terms.

## What librepaper.org stores

librepaper.org, your files and the backups are hosted in Canada by OVHcloud. Connections to librepaper.org use HTTPS.

- **Accounts** keep the sign-in provider and account ID, a handle, a display name, an email address (Google only), and when the account was created and last used.
- **Documents** keep the title, format, owner, dates, every file and the edit history.
- **Comments, highlights and replies** keep their author and the passage they refer to.
- **Project chat** keeps each message with its text, the author's name and the time it was sent.
- **Sharing** records who each document is shared with. A share link is stored as a hash and as an encrypted copy that lets the owner copy it again.
- **Agent tokens** are stored like share links, with their name, permission, comment limit and expiry.
- **Git links** keep a label for the repository, the branch and folder, the last commit that was synced, and who connected, pulled and pushed, and when. The repository's address and your Git credentials stay on your computer.
- **Moderation records** are written when an administrator blocks or erases an account, bans a handle, or hides a project. Each one keeps the administrator's login, the action, the date, the reason, and the sign-in provider and handle of the account or the project it concerns.
- **Operational numbers** count requests, response times, memory, disk and CPU once a minute. They carry no user, document or network identifiers. Only the administrators can see them.

librepaper.org keeps no access log and does not store IP addresses. The server uses your IP address, in memory only, to limit how many requests and connections one address can make. It is never written to the database or to a log. Server logs record errors and administrator sign-ins.

## Who sees what

> **Warning** Anyone who can read a document can also read its source files in
> the browser. Keep confidential inputs out of the project.

- Your handle is your GitHub login or your Google email. Only you see it. Other people see your display name.
- While you have a document open, other people viewing it see that you are there and where your cursor is.
- Edit and comment links require sign-in. Read links work for anyone who has them.
- Project chat is visible to people with a comment or edit link, and to the owner. Read links never show it.
- Administrators are able to read every account and document. They open a document only to act on an abuse report or a suspected breach of the [terms of service](terms.html), to meet a legal obligation, to fix a fault, or when its owner asks for help.

## What leaves librepaper.org

- **Hosting:** OVHcloud runs the machines and the storage that hold librepaper.org, your files and the backups, all in Canada.
- **Sign-in:** GitHub or Google sees your sign-in and tells librepaper.org who you are. Neither one sees your documents.
- **LaTeX mirror:** Your browser downloads the typesetting engines and LaTeX packages from the LibrePaper mirror. The mirror sees your IP address and the files you request, which can reveal the packages a document uses. It never sees the source.
- **Desktop app:** The [desktop app](desktop-app.html) runs on your computer. It sends local feature requests for your browser, and the browser never contacts your computer directly.
- **Writing assistant:** An assistant you start runs on your computer through the desktop app. Its access covers one document and expires after five minutes. The browser renews it while the page is open. The AI service behind the assistant sees what the assistant reads.
- **Local backups:** The desktop app downloads your projects, including those shared with you, and writes ZIP files to a folder you choose. The browser never learns where that folder is. The ZIP files stay on your computer when you turn backups off or delete a project. See [local backups](backups.html).
- **Moderation review:** When a document is reported, or the operator suspects that it breaks the [terms of service](terms.html), the operator may send its files to a third party for a first assessment. That third party is a moderation or copyright specialist, or the provider of a large language model. It sees the document, and it handles it under its own terms. Documents are not sent for any other reason.
- **Email:** A message you send to admin@librepaper.org is ordinary email. It is kept in the administrator's mailbox, not on librepaper.org.
- **Embedded resources:** A document can load images, fonts, scripts or data from other websites. Those sites see your IP address, your browser and when you opened the document, so an author can learn who read it. Authors can avoid this with PDFs or `embed-resources: true` in Quarto.

## Cookies and browser storage

librepaper.org sets two cookies. Each one is HttpOnly, Secure and SameSite=Lax, and carries the `__Host-` prefix.

- `librepaper_session` keeps you signed in for 30 days.
- `librepaper_state` ties a sign-in to the browser that started it. It lasts ten minutes.

The browser also keeps data of its own on your device.

- **Settings** remember your recent documents, layout and editor keys.
- **Caches** hold the typesetting engines, LaTeX packages, figures and your last preview.
- **Projects you open as an editor** are saved in the browser, so your edits survive a lost connection. Clear the site's data to remove them from a shared computer.
- **Share link keys** keep a link working in this browser. The browser sends the key with each request for its document.

## Data retention

- **Accounts** that have not signed in for 365 days are erased. Every sign-in resets the clock. See [erasing an account](#erasing-an-account).
- **Documents** stay until their owner deletes them or the owner's account is erased.
- **Deleted projects** stay in the trash for seven days. Then the project is purged with its files, comments, replies, chat, history and share links. Emptying the trash purges it at once.
- **Edit history and named versions** stay as long as the document exists. An owner can trim history, which permanently removes older history and named versions. When the owner trims, it also removes project chat. An editor's trim leaves chat. See [storage and history](./architecture/document.html#storage-and-history).
- **Project chat** stays until the owner clears it, the owner trims the project's history, or the owner deletes the project. Chat counts toward the owner's storage. Clearing chat deletes it at once.
- **Share links** expire after seven days unless the owner picks a longer time or no expiry.
- **Agent tokens** stop working when they expire or are revoked. Their records are removed when the project is purged.
- **Git links** stay until someone disconnects the project or the project is purged.
- **Moderation records** have no expiry. They stay after the account or project they concern is gone.
- **Sessions** end after 30 days. Signing out clears this browser's cookie but does not cancel a copied one. Erasing the account ends every session.
- **Operational numbers** are kept for 400 days and are not backed up.

## Backups

- A backup runs every hour. It holds the database and every stored file.
- Backups are encrypted and stored in Canada, away from the server.
- Every backup from the last 48 hours is kept, plus the last backup of each of the last 14 days and of each of the last 11 weeks. The oldest backup is therefore about ten weeks old.
- A deleted project is purged seven days after you delete it, so it can stay in backups for about eleven weeks. An erased account stays about as long.
- Cleared or trimmed chat can stay in backups until those backups expire, about ten weeks after the change.
- If backups stop running for a while, the oldest ones are kept longer.

## Erasing an account

Choose **Settings > Account > Erase this account** and confirm with your handle.

1. The account's sessions end, and the account can never sign in again. You cannot undo this from the browser.
2. Owned documents are marked for deletion and purged after seven days.
3. Comments, replies, labels and chat messages on other people's documents stay, shown as "Deleted user".
4. The account record is deleted once its documents are purged.

librepaper.org also erases an account that has not signed in for 365 days, in the same four steps. Signing in again later creates a new account with only the starter project.

The account stays in [backups](#backups) until they expire. If an administrator blocked or erased the account, the [moderation record](#what-librepaperorg-stores) stays, with the account's sign-in provider and handle.

## Your data, your requests

- **See it:** Your documents, comments and chat are in the app. For anything else librepaper.org holds about you, email the address below.
- **Correct it:** Your handle, display name and email address are copied from GitHub or Google each time you sign in. Change them there, then sign in again.
- **Take it with you:** Choose **Download project** to save a project's files, or let the desktop app keep [local backups](backups.html) of all of them.
- **Delete it:** Delete a project, or [erase your account](#erasing-an-account).

If you cannot do one of these yourself, or you think your data was handled badly, email [admin@librepaper.org](mailto:admin@librepaper.org).

## Changes to this page

This page is kept in the LibrePaper repository, and [its history](https://github.com/LibrePaper/librepaper/commits/main/docs/privacy.md) shows every change and its date.

## Contact

Email [admin@librepaper.org](mailto:admin@librepaper.org) with questions about your data.
