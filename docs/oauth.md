# Signing in to Microsoft 365 and Gmail

Microsoft 365 and Gmail do not take a password from a mail program. You sign in to the provider in your browser, and the provider hands Quill a sign-in it can use from then on. Quill never sees your password, and keeps the sign-in in the macOS Keychain.

For this the provider has to know which application is asking. Quill does not ship with an application id of its own: you register one, once, and put its id in `config.toml`. It takes about ten minutes for each provider and costs nothing.

Nothing here has been checked against the real services yet. The steps follow the providers' documentation as it stood when this was written; if a screen has moved, the names of the settings are what to look for.

## Microsoft 365 and Outlook.com

Quill reads and sends this mail through Microsoft Graph.

### Register the application

1. Go to <https://entra.microsoft.com> and sign in. A work account may need its administrator's leave to register applications; a personal Microsoft account can use <https://portal.azure.com> the same way.
2. Open **Applications**, then **App registrations**, then **New registration**.
3. Give it a name, such as "Quill".
4. Under **Supported account types** choose "Accounts in any organizational directory and personal Microsoft accounts", unless you only ever want accounts of your own organisation.
5. Under **Redirect URI** choose the platform **Public client/native (mobile & desktop)** and enter `http://localhost`. Just that: no port and no path.
6. Press **Register**. The page that follows shows the **Application (client) ID**. That is what goes in the config.
7. Open **Authentication**. Under **Advanced settings** turn on **Allow public client flows**. Signing in with a code from another computer needs it.
8. Open **API permissions**, then **Add a permission**, **Microsoft Graph**, **Delegated permissions**, and add `Mail.ReadWrite`, `Mail.Send`, `User.Read` and `offline_access`.

Do not create a client secret. A desktop application cannot keep one, and Microsoft does not ask a public client for one.

### The account in config.toml

```toml
[[account]]
name = "Work"
type = "graph"
address = "you@example.com"
oauth_client_id = "the Application (client) ID"
```

If you chose a single organisation in step 4, name it too, by its id or one of its domain names:

```toml
oauth_tenant = "example.com"
```

### Sign in

Start Quill. The account is listed, with a note that nobody has signed in to it yet. Press `S` (or choose "Sign in to an account" from the palette or the File menu). Your browser opens at Microsoft's sign-in page; sign in there and agree to what Quill asks for, and come back to Quill. The mail starts to arrive.

From a terminal the same thing is `quill account sign-in Work`.

Over SSH there is no browser to open on the Mac you are sitting at. Quill shows a short code and an address instead: open the address on any device, type the code in, and sign in there. `quill account sign-in Work --code` asks for this way at the Mac itself.

## Gmail

Quill reads Gmail over IMAP and sends over SMTP, signing in to both with the same sign-in.

An app password is the other way to use Gmail, and needs none of this: turn on 2-Step Verification, create an app password at <https://myaccount.google.com/apppasswords>, and add the account in Quill with that as its password.

### Register the application

1. Go to <https://console.cloud.google.com> and create a project. Any name will do.
2. Under **APIs & Services**, **Library**, find the **Gmail API** and enable it.
3. Under **APIs & Services**, **OAuth consent screen**, set the application up as **External**, give it a name and your address, and add the scope `https://mail.google.com/`.
4. While the application is in **Testing**, add your own Gmail address under **Test users**. Only test users can sign in.
5. Under **APIs & Services**, **Credentials**, choose **Create credentials**, **OAuth client ID**, and the application type **Desktop app**.
6. The page that follows shows a **Client ID** and a **Client secret**. Both go in the config.

Two things to know. Google calls the second value a secret, and says itself that for a desktop application it is not one: every copy of the application carries it. That is why it may sit in `config.toml`, where real passwords never do. And while the application is in Testing, Google ends each sign-in after seven days, so Quill will ask you to sign in again every week. Publishing the application ends that, and for the mail scope Google wants to verify it first.

### The account in config.toml

```toml
[[account]]
name = "Personal"
type = "imap"
address = "you@gmail.com"
auth = "oauth"
oauth_client_id = "the Client ID"
oauth_client_secret = "the Client secret"
imap = { host = "imap.gmail.com", port = 993, tls = "implicit" }
smtp = { host = "smtp.gmail.com", port = 465, tls = "implicit" }
```

### Sign in

As for Microsoft: `S` in Quill, or `quill account sign-in Personal`. Google has no sign-in by code for mail, so this has to be done at the Mac itself.

## Looking after a sign-in

- `quill account list` says for each account whether it is signed in.
- `quill account forget NAME` removes the sign-in from the Keychain. Quill then asks for a new one.
- The sign-in is kept under your address, not the account's name, so renaming an account in the config keeps it.
- Removing Quill's access at the provider (Microsoft: <https://myaccount.microsoft.com>, under privacy and applications; Google: <https://myaccount.google.com/permissions>) ends the sign-in everywhere. Quill notices at its next sync and asks you to sign in again.

## A national cloud

Microsoft's national clouds have their own addresses. Give them in the account:

```toml
[account.oauth]
authorize_url = "https://login.microsoftonline.us/common/oauth2/v2.0/authorize"
token_url = "https://login.microsoftonline.us/common/oauth2/v2.0/token"
device_url = "https://login.microsoftonline.us/common/oauth2/v2.0/devicecode"
graph_url = "https://graph.microsoft.us/v1.0"
```

Each must be `https`.
