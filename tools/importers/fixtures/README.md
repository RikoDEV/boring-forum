# Test forums for the importers

Real phpBB and MyBB installs, filled with content, to export from and import. Nothing here needs a real
site's data. Needs Docker. Everything is built from the products' own code, so posts are stored as they
really are (phpBB 3.3 keeps them as XML).

```sh
docker build -t fx-php tools/importers/fixtures                 # PHP 8.3 with mysqli and gd
docker network create fx
docker run -d --name fx-db --network fx -e MARIADB_ROOT_PASSWORD=root -p 127.0.0.1:3307:3306 mariadb:11
docker exec fx-db mariadb -uroot -proot -e "CREATE DATABASE phpbb CHARACTER SET utf8mb4"

# phpBB 3.3: fetch it, install its dependencies and run its command-line installer
git clone --depth 1 --branch release-3.3.19 https://github.com/phpbb/phpbb phpbb-src
docker run --rm -v "$PWD/phpbb-src:/src" -w /src/phpBB fx-php composer install --no-dev --no-interaction
docker run --rm --network fx -v "$PWD/phpbb-src:/src" -v "$PWD/tools/importers/fixtures/phpbb/install.yml:/tmp/install.yml" \
  -w /src/phpBB fx-php php install/phpbbcli.php install /tmp/install.yml
docker exec fx-db mariadb -uroot -proot phpbb -e "UPDATE phpbb_config SET config_value=0 WHERE config_name='board_disable'"
rm -rf phpbb-src/phpBB/cache/production/*

# fill it: 6 members, forums (one staff-only), polls, attachments, avatars, private messages, a phpBB 3.0 hash
docker run --rm --network fx -v "$PWD/phpbb-src:/src" -v "$PWD/tools/importers/fixtures/phpbb:/fx" \
  -w /src/phpBB fx-php php /fx/seed.php

# export, then import with the phpBB folder as the files folder
PHPBB_DB="mysql://root:root@127.0.0.1:3307/phpbb" node tools/importers/export-phpbb.mjs out.json
BF_IMPORT_FILES="$PWD/phpbb-src/phpBB" boring-forum import out.json
```

The seed script writes into the phpBB folder (`files/`, `images/avatars/upload/`), so keep it out of git.

## MyBB 1.8

```sh
git clone --depth 1 --branch mybb_1841 https://github.com/mybb/mybb mybb-src
docker exec fx-db mariadb -uroot -proot -e "CREATE DATABASE mybb CHARACTER SET utf8mb4"
# serve it and run its web installer (the script fills in each step's form)
docker run -d --name fx-mybb --network fx -p 127.0.0.1:8001:8000 -v "$PWD/mybb-src:/app" -w /app fx-php php -S 0.0.0.0:8000 -t /app
python tools/importers/fixtures/mybb/install.py

# fill it: 6 members, forums (one staff-only), polls, attachments, avatars, private messages
docker exec fx-db mariadb -uroot -proot mybb -e "DELETE FROM mybb_forums; DELETE FROM mybb_users WHERE uid>1"
python tools/importers/fixtures/mybb/seed.py mybb-src seed.sql
docker exec -i fx-db mariadb -uroot -proot mybb < seed.sql

MYBB_DB="mysql://root:root@127.0.0.1:3307/mybb" node tools/importers/export-mybb.mjs out.json
BF_IMPORT_FILES="$PWD/mybb-src" boring-forum import out.json
```

MyBB stores post text as plain BBCode, so its seed is SQL; the seed also writes the avatar and attachment files into `mybb-src/uploads/`.
