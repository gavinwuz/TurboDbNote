# Driver identity artwork

These marks identify the selected database vendor/project; they do not indicate sponsorship or endorsement. Trademarks remain with their respective owners.

`sources.json` records each source URL, origin and SHA-256. Vendor/project-hosted artwork is preferred. MySQL, SQLite, SQL Server, MongoDB and Elasticsearch currently use Devicon mirrors of brand artwork; the Devicon license is included. IBM Db2 uses IBM's Carbon product icon, rendered in IBM blue (Apache-2.0; https://github.com/carbon-design-system/carbon).

Original sources are retained under `sources/`. PNGs preserve aspect ratio and color, trimming only transparent margins, and are embedded in the application. No image is downloaded at runtime. Regeneration uses `python scripts/generate-driver-logos.py` with the same resvg-py/Pillow tools used for application-icon maintenance; normal compilation needs neither tool.
