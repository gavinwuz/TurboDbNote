# Windows installer sources

`turbodbnote.iss` is compiled with Inno Setup 6.7.3. Keep AppId fixed across upgrades. Target baseline: Windows 10 1809+ and Windows 11 x64. AppMutex matches the application's lifetime marker; never enable forced application shutdown while edits are in memory.

`ChineseSimplified.isl` is the translation by Zhenghan Yang listed on the official Inno Setup translations page, downloaded from https://raw.githubusercontent.com/jrsoftware/issrc/refs/heads/main/Files/Languages/ChineseSimplified.isl on 2026-09-11. Original notices are retained. See `INNO-LICENSE.txt` for the Inno Setup source redistribution license. Compilation does not fetch this file from the network.

The package uses current-user installation, no automatic launch, no user-data deletion rules and no registry file associations. Upgrade/downgrade migration and signing require separate release acceptance. `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART` is supported for deployment; the running-app mutex must be absent first.
