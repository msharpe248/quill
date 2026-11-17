# Quill and your privacy

Quill is a mail program that runs on your Mac. This page says what it does with your mail and your accounts. It applies to Quill as its author gives it out; a copy someone else has changed is theirs to answer for.

## In short

Quill has no servers of its own. Your mail, your passwords and your sign-ins stay on your Mac, and go nowhere but to the mail providers you have set Quill up with. Nothing is sent to Quill's author or to anyone else, and Quill has no analytics, no advertising and no tracking of any kind.

## What Quill keeps, and where

- **Your mail and your settings** are files on your Mac, in `~/Library/Application Support/Quill`. Removing an account in Quill deletes that account's mail from there.
- **Your passwords and sign-ins** are kept in the macOS Keychain and nowhere else: not in the settings file, not in the mail database, not in a log.

Nothing of this is copied anywhere by Quill. Backing it up, or not, is up to you and to whatever backs up your Mac.

## What Quill connects to

Only what you have asked it to:

- **Your mail providers**, to read, send and file your mail: the mail servers of each account you add, or Microsoft's and Google's services for accounts that sign in with them.
- **Your provider's sign-in page**, in your own browser, when you sign in to an account with Microsoft or Google. Quill never sees the password you type there.
- **The name servers your Mac already uses**, to find the mail servers for an address when you add an account.
- **A mailing list's own server**, once, when you ask Quill to unsubscribe you from that list.
- **A link or a remote picture in a message**, only when you open the link or ask for the pictures. Pictures in mail are not loaded until you ask for them, for that message or, if you say so, always for that sender.

Quill does not check for updates, and does not report crashes or how it is used.

## Google accounts

When you sign in to a Gmail or Google Workspace account, Quill asks Google for access to that account's mail (`https://mail.google.com/`). It uses that access for one thing: to show you your mail and to send, move, mark and delete mail as you direct, on your Mac.

- Your Google data is read from Google by the copy of Quill on your Mac and is stored only on that Mac.
- It is not sent to Quill's author or to any other party.
- It is not used for advertising, is not sold, and is not used to train AI or machine-learning models.
- Quill has no AI features: it does not use Google's services or your Google data to make, change or analyse pictures, video, sound or text with AI.
- No person other than you reads it.

Quill's use of information received from Google APIs adheres to the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including the Limited Use requirements.

You can take Quill's access away at any time at <https://myaccount.google.com/permissions>, and remove the account and its mail from Quill in Settings, Accounts.

## Microsoft accounts

The same holds for accounts that sign in with Microsoft: Quill reads and sends that account's mail from your Mac, keeps it only on your Mac, and shares it with no one. Access can be removed at <https://myaccount.microsoft.com>.

## Questions

Open an issue on the page Quill is published from: <https://github.com/msharpe248/quill/issues>.

*Last changed: 17 November 2026.*
