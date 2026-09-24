# World's Fair 60s demo script

**~60 seconds, spoken**

Hi — this is karat67. Indexers can look perfectly healthy while writing garbage.

**Pass.** Run shape on UserMetadata at the correct length — 1032 bytes. The check passes. Your pipeline can gate on that.

**Fail.** Same account type, zero-byte payload — the silent corruption class we saw in production. Shape fails. Your liveness and freshness dashboards can stay green the whole time; that's why shape exists.

Run `bash demo/worlds_fair.sh` for both steps and acceptance.
