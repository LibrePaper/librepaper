---
title: "Privacy and data retention"
---

> **Warning:** Do not store private or highly sensitive data on librepaper.org.
> Documents are not end-to-end encrypted. The server reads every file so it can
> merge edits from people who write at the same time. The administrators must
> also be able to read files to moderate them. If you need private file
> handling, [host your own server](./host/simple.html).

## What librepaper.org stores

librepaper.org and your files are hosted in Canada.

- **Accounts** keep the sign-in provider and account ID, a handle, a display name, an email address (Google only), and when the account was created and last used.
- **Documents** keep the title, format, owner, dates, every file and the edit history.
- **Comments, highlights, suggestions and replies** keep their author and the passage they refer to.
- **Sharing** records who each document is shared with. A share link is stored as a hash and as an encrypted copy that lets the owner copy it again.
- **Operational numbers** count requests, response times, memory, disk and CPU once a minute. They carry no user, document or network identifiers. Only the administrators can see them.

librepaper.org keeps no access log and does not store IP addresses.

## Who sees what

- Your handle is your GitHub login or your Google email. Only you see it. Other people see your display name.
- While you have a document open, other people viewing it see that you are there and where your cursor is.
- Edit and comment links require sign-in. Read links work for anyone who has them.
- Anyone who can read a document can also read its source files in the browser. Keep confidential inputs out of the project. The [local app](local-app.html) can supply local files to a build without uploading them.

## What leaves librepaper.org

- **Sign-in:** GitHub or Google sees your sign-in and tells librepaper.org who you are. Neither one sees your documents.
- **LaTeX mirror:** Your browser downloads the typesetting engines and LaTeX packages from the LibrePaper mirror. The mirror sees your IP address and the files you request, which can reveal the packages a document uses. It never sees the source.
- **Local app:** The [local app](local-app.html) runs on your computer, and the browser talks to it directly. The browser contacts it when you use a local feature. Once you have paired it, the browser also asks it for backup status.
- **Writing assistant:** An assistant you start runs on your computer through the local app. Its access covers one document and expires after five minutes. The browser renews it while the page is open. The AI service behind the assistant sees what the assistant reads.
- **Local backups:** The local app downloads your projects, including those shared with you, and writes ZIP files to a folder you choose. The browser never learns where that folder is. The ZIP files stay on your computer when you turn backups off or delete a project. See [local backups](backups.html).
- **Embedded resources:** A document can load images, fonts, scripts or data from other websites. Those sites see your IP address, your browser and when you opened the document, so an author can learn who read it. Authors can avoid this with PDFs or `embed-resources: true` in Quarto.

## Cookies and browser storage

librepaper.org sets two cookies. Each one is HttpOnly, Secure and SameSite=Lax, and carries the `__Host-` prefix.

- `librepaper_session` keeps you signed in for 30 days.
- `librepaper_state` ties a sign-in to the browser that started it. It lasts ten minutes.

The browser also keeps data of its own on your device.

- **Settings** remember your recent documents, layout, editor keys and local app pairing.
- **Caches** hold the typesetting engines, LaTeX packages, figures and your last preview.
- **Projects you open as an editor** are saved in the browser, so your edits survive a lost connection. Clear the site's data to remove them from a shared computer.
- **Share link keys** keep a link working in this browser. The browser sends the key with each request for its document.

## Data retention

- **Documents** stay until their owner deletes them.
- **Deleted projects** stay in the trash for seven days. Then the project is purged with its files, comments, replies, history and share links. Emptying the trash purges it at once.
- **Edit history and named versions** stay as long as the document exists. An owner can trim history, which permanently removes older history and named versions. See [storage and history](./architecture/document.html#storage-and-history).
- **Share links** expire after seven days unless the owner picks a longer time or no expiry.
- **Sessions** end after 30 days. Signing out clears this browser's cookie but does not cancel a copied one. Erasing the account ends every session.
- **Operational numbers** are kept for 400 days and are not backed up.

## Backups

- A backup runs every day at midnight UTC. It holds the database and every stored file.
- Backups are encrypted and stored in Canada, away from the server.
- Every backup from the last 48 hours is kept. After that, one backup per day is kept for 14 days and one per week for 11 weeks.
- Deleted documents and erased accounts therefore stay in backups for about three months.
- After a restore, erasures made since that backup are applied again.

## Erasing an account

Choose **Settings > Account > Erase this account** and confirm with your handle.

1. The account's sessions end, and the account can never sign in again. You cannot undo this from the browser.
2. Owned documents are marked for deletion and purged after seven days.
3. Comments, replies and labels on other people's documents stay, shown as "Deleted user".
4. The account record is deleted once its documents are purged.

The account stays in [backups](#backups) until they expire.
