FROM php:8.3-cli
RUN apt-get update && apt-get install -y --no-install-recommends libpng-dev libjpeg-dev libfreetype-dev libzip-dev unzip git && rm -rf /var/lib/apt/lists/* \
 && docker-php-ext-configure gd --with-freetype --with-jpeg && docker-php-ext-install -j2 mysqli gd zip pdo_mysql
COPY --from=composer:2 /usr/bin/composer /usr/local/bin/composer
