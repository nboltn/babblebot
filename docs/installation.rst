============
Installation
============

Install rust on your system: https://www.rust-lang.org/tools/install

I use the nightly branch::

    $ rustup default nightly

Clone the repo and run the build script::

    $ git clone https://gitlab.com/toovs/babblebot
    $ cd babblebot
    $ ./scripts/build.sh

Copy the .example files and modify as needed::

    $ cp Rocket.toml.example Rocket.toml
    $ cp Settings.toml.example Settings.toml

Run the bot::

    $ ./bin/babblebot
