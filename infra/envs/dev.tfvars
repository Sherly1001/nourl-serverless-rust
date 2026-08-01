domain_name        = "dev.nourl.space"
mongo_url_ssm_path = "/nourl-dev/mongo-url"

# Same Atlas cluster as prod, different database — the two SSM parameters are
# currently identical, so without this dev would read and write live data.
mongo_db = "nourl-dev"
