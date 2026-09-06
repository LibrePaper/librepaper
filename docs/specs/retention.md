# Storage Retention and Contact Policy

## 1. Goals

The service provides the same functionality to free and paid users. Payment is only for additional storage.

The retention system should:

- provide useful free storage without allowing abandoned accounts to accumulate forever;
- avoid deleting files belonging to active users;
- give users advance notice before automatic deletion;
- use GitHub as the authority for identity and email addresses;
- avoid maintaining a separate user-editable email system;
- keep the policy simple enough for users to understand.

## 2. Storage tiers

The free storage quota is:

- 100 MB per user.

A paid user receives a larger storage quota, but no additional application features.

Paid storage is retained while the paid subscription remains active and is not subject to the free-storage inactivity cleanup described below.

The paid-plan quota and price are independent configuration values and are outside the scope of this specification.

## 3. Contact email

The application requests permission to read the user's GitHub email addresses.

For an OAuth App, this requires the `user:email` scope. GitHub's authenticated-user email endpoint returns information including whether an address is `primary` and `verified`.

The application's contact email is the GitHub email for which:

```text
primary = true
verified = true
```

The application does not provide an interface for changing this address.

If a user wants to change their contact email, they must change and verify their primary email address on GitHub. The application will synchronize the new address on a subsequent login.

## 4. Email synchronization

On every successful GitHub login:

1. Fetch the user's GitHub identity.
2. Fetch the user's email addresses.
3. Find the primary, verified email address.
4. Compare it with the address currently stored by the application.
5. If it differs, replace the stored address with the new GitHub primary verified address.
6. Record the time at which the email information was last synchronized.

GitHub users can modify the permissions granted to an OAuth application, so access to `user:email` must not be assumed to remain available forever. GitHub recommends handling reduced or changed scopes gracefully.

If the application cannot obtain a primary verified GitHub email during login, the user should be told that a verified GitHub email accessible to the application is required for free persistent storage.

The user should be directed to GitHub to correct the problem or reauthorize the required permission.

## 5. Email reachability check

A GitHub `verified` email establishes that GitHub has verified the address. It does not guarantee that the mailbox will remain reachable indefinitely.

The application therefore performs its own delivery check without creating a separate email-verification system.

When a user first becomes eligible for persistent storage, send a transactional email to the GitHub-provided address.

The email does not need to contain a confirmation link. Its purposes are to:

- tell the user which address will receive important storage notices;
- verify that mail can currently be delivered;
- allow the email provider to report a hard bounce.

Suggested message:

> This is the email address associated with your GitHub account. We will use it only for important service messages, including warnings before inactive free storage is deleted.

If the primary GitHub email changes, send the same notification to the new address.

## 6. Delivery status

Store at least:

```text
github_user_id
contact_email
contact_email_synced_at
contact_email_delivery_status
contact_email_last_success_at
contact_email_last_failure_at
```

Possible delivery states:

```text
unknown
deliverable
hard_bounce
```

A successfully accepted delivery may mark the address as `deliverable`.

A hard bounce marks it as `hard_bounce`.

Temporary delivery failures should not immediately mark an address permanently unusable.

The exact bounce/retry implementation depends on the transactional email provider.

## 7. Undeliverable email

If an email hard-bounces, the application must not allow the user to substitute an arbitrary email address.

Instead, when the user next signs in:

1. Query GitHub again.
2. Check the current primary verified email.
3. If GitHub now supplies a different address, update the stored address and test delivery again.
4. If GitHub still supplies the undeliverable address, tell the user to update their primary email on GitHub.

The application remains dependent on GitHub for contact identity.

## 8. Definition of activity

Automatic cleanup applies to free storage based on owner activity.

Activity is based on the authenticated owner using the service, not on whether individual documents receive views.

At minimum, a successful authenticated login counts as activity and resets the inactivity timer.

Other authenticated owner actions may also count, such as:

- uploading a file;
- creating a document;
- modifying a document;
- deleting a document;
- commenting;
- changing project metadata.

The implementation should maintain a single timestamp:

```text
last_activity_at
```

Any qualifying authenticated action updates it.

Anonymous document views do not update it.

Views by other users do not update it.

Search-engine crawlers and bots do not update it.

This avoids abandoned public documents being retained forever merely because they continue receiving traffic.

## 9. Free-storage inactivity period

Free storage becomes eligible for deletion after:

```text
6 months
```

of owner inactivity.

The clock begins from `last_activity_at`.

Any qualifying activity before deletion resets the entire six-month clock.

Example:

```text
January 1    last activity
June 1       approximately 5 months inactive; first warning
June 24      approximately 7 days remain; final warning
July 1       six months inactive; storage eligible for deletion
```

Exact calendar calculations should be used rather than assuming every month has 30 days.

## 10. Warning emails

Before automatically deleting free storage, send at least two warnings.

### First warning

Send approximately 30 days before scheduled deletion.

The message should state:

- that the account's free storage has been inactive;
- the scheduled deletion date;
- that stored files will be deleted;
- that simply signing in before that date will preserve the files and reset the inactivity period.

### Final warning

Send approximately 7 days before scheduled deletion.

Repeat:

- the exact scheduled deletion date;
- what will be deleted;
- that signing in cancels the pending cleanup.

The application may also send a deletion notice after deletion.

## 11. No inactivity banner

Do not show an inactivity-warning banner after the user signs in.

Signing in itself counts as activity and therefore cancels the pending deletion. A banner saying that deletion is imminent would immediately become obsolete.

The retention policy and current activity status may still be displayed normally in account/storage settings.

For example:

```text
Free storage is retained while your account is active.
If you do not use the service for 6 months, your stored
files may be deleted. We will email you before deletion.
```

## 12. Cancellation of scheduled deletion

Before performing any deletion, check `last_activity_at` again.

Never rely solely on a previously scheduled deletion job.

Conceptually:

```text
if user_is_free
    and now >= last_activity_at + 6 months:
        delete_free_storage()
```

If the user has returned since the warning was generated, deletion must not occur.

This protects against races between login/activity events and scheduled cleanup jobs.

## 13. What gets deleted

The inactivity policy applies to stored user content, not necessarily the user's identity record.

Deletion should include, as appropriate:

- uploaded files;
- generated files counted as persistent user storage;
- document revisions/history;
- stored binary assets;
- other objects associated with the user's storage quota.

The application may retain the minimal account record necessary to recognize the GitHub identity if the user returns later.

The user should therefore be able to sign in again after cleanup and begin with empty storage.

## 14. Recovery period

After logical deletion, deleted content may remain recoverable from backup for a short grace period.

Recommended grace period:

```text
30 days
```

During this period:

- files are no longer available normally;
- support or an automated restore mechanism may restore them;
- the backup copy must eventually be purged.

After the recovery period expires, the data should be permanently removed from the backup system according to the backup-retention policy.

The user-facing policy should distinguish clearly between:

```text
scheduled deletion
logical deletion
permanent deletion
```

Do not promise recovery unless the implementation actually supports it.

## 15. Email failure does not prevent cleanup indefinitely

Email delivery cannot be guaranteed.

The service should make a reasonable effort to send the required warnings to the most recently synchronized primary verified GitHub email.

An undeliverable warning should be recorded, but should not cause abandoned free storage to be retained forever.

Otherwise, users with obsolete email addresses would defeat the purpose of the inactivity policy and abandoned storage would continue accumulating indefinitely.

The retention terms should therefore make clear that:

- users are responsible for maintaining a current primary verified email with GitHub;
- users are responsible for allowing the application access to that address;
- email notifications are a courtesy/best-effort safeguard;
- the six-month inactivity rule ultimately determines retention.

## 16. Recommended deletion-state model

A simple implementation could use:

```text
ACTIVE
WARNING_30D
WARNING_7D
DELETED_RECOVERABLE
PURGED
```

However, these states should preferably be derived from timestamps rather than treated as the authoritative source of truth.

Useful fields include:

```text
last_activity_at

retention_warning_30d_sent_at
retention_warning_7d_sent_at

storage_deleted_at
storage_purge_after

contact_email
contact_email_synced_at
contact_email_delivery_status

storage_plan
```

When activity resumes, warning timestamps can be cleared for the new inactivity cycle.

## 17. Cleanup job

Run a scheduled cleanup process, for example daily.

For each free account:

```text
1. Calculate deletion_at = last_activity_at + 6 months.

2. If deletion_at is about 30 days away and the 30-day
   warning has not been sent for this inactivity cycle:
       send warning.

3. If deletion_at is about 7 days away and the 7-day
   warning has not been sent for this inactivity cycle:
       send warning.

4. If now >= deletion_at:
       reload the user's current state;
       confirm the user is still on the free plan;
       confirm last_activity_at still qualifies;
       delete the user's stored content;
       record storage_deleted_at;
       establish the recovery/purge deadline.

5. When the recovery deadline passes:
       purge recoverable backup data.
```

The process must be idempotent so rerunning a failed job cannot cause unexpected duplicate deletion or notifications.

## 18. Paid users

Users with an active paid storage plan are excluded from inactivity cleanup.

Their files remain stored regardless of login frequency while the storage subscription remains active.

A separate specification should define what happens when:

- payment fails;
- a subscription expires;
- a paid user cancels;
- stored data exceeds the free 100 MB quota when the account returns to the free tier.

Those cases should not be silently handled by the general inactivity rule.

## 19. User-facing summary

The policy should be communicated in simple terms:

> All features are free. Free accounts include 100 MB of storage. To prevent abandoned storage from accumulating indefinitely, files in free accounts may be deleted after 6 months without activity. Signing in resets the inactivity period. We will send advance warnings to the primary verified email address associated with your GitHub account. If you change your email address, update it on GitHub.

## 20. Design principles

The implementation should preserve the following principles:

1. Features are never gated by payment.
2. Payment buys storage only.
3. Active free users keep their 100 MB indefinitely.
4. Abandoned free storage does not accumulate indefinitely.
5. Owner activity, not document traffic, determines retention.
6. GitHub remains the authority for both identity and contact email.
7. The application does not maintain user-editable email addresses.
8. GitHub email information is refreshed whenever the user signs in.
9. The application checks actual email delivery and records hard bounces.
10. Users receive advance deletion warnings whenever deliverable email allows it.
11. Returning before deletion immediately resets the inactivity clock.
12. Deletion jobs always re-check current state immediately before deleting.
13. Email delivery failure cannot create permanent free storage.
14. Paid storage is not subject to inactivity cleanup while payment remains active.