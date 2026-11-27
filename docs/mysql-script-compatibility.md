# MySQL script compatibility scope

The earlier MySQL 4.1 SkillTool migration proposal is withdrawn. Supported direction is same-version import or import into a newer MySQL server; newer versions do not guarantee cross-major syntax/collation compatibility.

Native file import freezes the input and uses one mysql process/session with reconnect disabled and no force/continue-on-error option. Standard mysqldump source-version headers are compared numerically against SELECT VERSION() before writes. Missing source headers do not establish a source version and cannot certify compatibility.

The shared MySQL splitter handles DELIMITER directives, including indentation and case differences, preserving compound routine bodies. Bounded MySQL attachment indexes reuse it and rebuild older indexes. The guarded Note execution classifier still rejects unsupported routine/dynamic SQL; use native whole-file import instead of skipping statements.

No global trust setting is modified. No business SQL is executed by verification. Actual MySQL 5.7 routine import remains unverified.

## Whole-file import route

The SQL attachment More menu exposes native whole-file import using the cell connection/database and the saved attachment. This opens the existing transfer review dialog; it does not execute immediately. SQL file paths can also be selected by dropping one .sql file onto the import file field. Dependent dynamic statements/routines remain rejected by the Note per-statement validator; the UI directs users to whole-file import instead of recommending skips. One native mysql process executes the frozen file, with reconnect disabled and no force flag. MySQL server permissions still apply.
