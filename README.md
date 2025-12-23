# Halo Action Importer
This application imports actions into Halo in mass. It intakes a directory of excel/csv files and imports based on unique identifer against the action skipping existing.

# Requirements
- Halo instance
- Actions to import in excel/csv formatted correctly per Halo API documentation (/api/apidoc)
- Actions MUST have a unique identifier outside of what Halo assigns to action entries at creation
    - This allows us to check which actions have already been imported
