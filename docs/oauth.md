# Signing in to Microsoft 365 and Gmail

Microsoft 365 and Gmail do not take a password from a mail program. You sign in to the provider in your browser, and the provider hands Quill a sign-in it can use from then on. Quill never sees your password, and keeps the sign-in in the macOS Keychain.

For this the provider has to know which application is asking. With Microsoft that is Quill itself: it is registered there, its application id is built in, and you only sign in. With Google there is no application of Quill's: you register one, once, and give Quill its id.

Nothing here has been checked against the real services yet. The steps follow the providers' documentation as it stood when this was written; if a screen has moved, the names of the settings are what to look for.

## Microsoft 365 and Outlook.com

Quill reads and sends this mail through Microsoft Graph.

To add a Microsoft account, press `A` in Quill, type the address and continue: Microsoft's sign-in page opens in your browser. The rest of this section is only for someone who wants Quill to sign in as an application of their own, such as an organisation that allows only applications it has registered.

### Register an application of your own

1. Go to <https://entra.microsoft.com> and sign in. A work account may need its administrator's leave to register applications; a personal Microsoft account can use <https://portal.azure.com> the same way.
2. In the menu on the left open **Entra ID**, then **App registrations**, then press **New registration**. (Older screens have it under **Applications**. If there is no App registrations at all, or you are told you have no directory, see the note below the list.)
3. Give it a name, such as "Quill".
4. Under **Supported account types** choose "Any Entra ID Tenant + Personal Microsoft accounts" (older screens: "Accounts in any organizational directory and personal Microsoft accounts"), unless you only ever want accounts of your own organisation. An Outlook.com or Hotmail address is a personal account, and needs this choice.
5. Under **Redirect URI** choose the platform **Public client/native (mobile & desktop)** and enter `http://localhost`. Just that: no port and no path.
6. Press **Register**. The **Overview** page that follows shows the **Application (client) ID**, a long code of letters, digits and dashes. That is the application id Quill asks for, and all it needs from this page.
7. Open **Authentication**. Under **Advanced settings** turn on **Allow public client flows**. Signing in with a code from another computer needs it.
8. Open **API permissions**, then **Add a permission**, **Microsoft Graph**, **Delegated permissions**, and add `Mail.ReadWrite`, `Mail.Send`, `User.Read` and `offline_access`.

An application is registered in a directory (a "tenant"). A work or school account has one. A personal Microsoft account may have none, and then the admin center has nowhere to put a registration: Microsoft's own answer is to create a directory first, which a free Azure account does. This guide has not been through that case.

Do not create a client secret. A desktop application cannot keep one, and Microsoft does not ask a public client for one.

### Use it for an account

An application of your own is named in the config, since the form adds Microsoft accounts as Quill itself. Write the account as below. If the config already has a Microsoft account that names an application, the form offers that id for the next one, with a tenant to give.

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

An account added in Quill goes to this by itself. For one written by hand, start Quill. The account is listed, with a note that nobody has signed in to it yet. Press `S` (or choose "Sign in to an account" from the palette or the File menu). Your browser opens at Microsoft's sign-in page; sign in there and agree to what Quill asks for, and come back to Quill. The mail starts to arrive.

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
6. The page that follows shows a **Client ID** and a **Client secret**. Quill asks for both.

Two things to know. Google calls the second value a secret, and says itself that for a desktop application it is not one: every copy of the application carries it. That is why it may sit in `config.toml`, where real passwords never do. And while the application is in Testing, Google ends each sign-in after seven days, so Quill will ask you to sign in again every week. Publishing the application ends that, and for the mail scope Google wants to verify it first.

### Add the account in Quill

Press `A`, type the Gmail address and continue. The form offers the account with a password, which for Google is an app password. Change **Type** to **Google (sign in)**: the form then asks for the Client ID and the Client secret in place of servers and a password. Continue, and Google's sign-in page opens in your browser; sign in there, agree to what Quill asks for, and come back.

The application is asked for once. The next Google account you add is offered the same one, and is a Google account from the start. Signing in with Google is for Google's own mail servers: a Gmail address, or an address of your own whose mail Google Workspace hosts.

If Quill is ever given an application of its own with Google, as it has with Microsoft, none of this section will be needed: a Gmail address will ask for nothing but the sign-in.

### The account in config.toml

What the form writes, and what to write by hand instead:

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
