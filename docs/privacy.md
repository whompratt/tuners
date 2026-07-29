# Telemetry sharing: privacy note

_tuners_ can optionally share driving telemetry to improve the advice engine
for everyone. This page is the plain-language record of what that means.
Sharing is **off by default**; nothing leaves your machine until you turn it
on (Settings → telemetry sharing), and you can turn it off again at any time.

## What is sent

When sharing is on, each finished run is packaged and uploaded as a bundle
containing:

- the raw driving telemetry the game broadcast (car physics only — speed,
  suspension, tire, engine and position channels; the game's Data Out
  stream contains no personal information),
- the car and the setup values needed to interpret it,
- the setup *changes* between runs (e.g. "front arb −2").

## What is never sent

- Names, gamertags, account details, or anything identifying you. The tool
  never has them in the first place.
- Free text you type anywhere in the app (run notes, project names,
  descriptions). The export format has no field for free text — stripping
  is structural, not a redaction pass that could miss something.
- Anything while sharing is off, and anything from projects you drove
  before turning it on (sharing history is its own separate, explicit
  action with a preview of exactly what would be sent).

## Identity

At opt-in the app generates a random token on your machine. Uploads are
grouped under a short fingerprint of that token (the "sender id" shown in
Settings). That id is pseudonymous: it lets the data from one driver hang
together (which the learning needs) and gives deletion requests something
to point at. It is not linked to your name, account, or IP address; the
receiving endpoint keeps no request logs tied to bundles.

## Where it goes, and retention

Bundles are stored privately (a cloud storage bucket controlled by the
project) and are used only for developing and calibrating this tool's
advice engine. They are not sold, shared onward, or published. Retention
policy:

- Bundles are kept while they usefully contribute to the tool's models,
  and reviewed for pruning after **24 months**.
- **Delete on request**: quote your sender id (Settings shows it) and all
  bundles under it are deleted. Since the id is the only link, deletion is
  complete.
- If the project winds down, the bucket is deleted, not handed over.

## Stopping

Toggle sharing off at any time. Queued bundles that haven't uploaded yet
prompt you to discard or send them; nothing new is collected from that
moment. Deleting the app's data directory also destroys your token, which
permanently orphans (and effectively anonymizes) anything already uploaded
— quote the sender id first if you want the data deleted too.
