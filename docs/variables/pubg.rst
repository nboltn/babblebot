pubg
----

return the number of pubg wins:

    (pubg:wins)

return the number of pubg kills:

    (pubg:kills)

return the amount of pubg damage dealt:

    (pubg:damage)

return the number of pubg headshots:

    (pubg:headshots)

return the number of pubg roadkills:

    (pubg:roadkills)

return the number of pubg teamkills:

    (pubg:teamkills)

return the number of pubg vehicles destroyed:

    (pubg:vehicles-destroyed)

.. rubric:: Required Settings

The ``pubg:name`` and ``pubg:token`` settings. The token is obtained from https://developer.pubg.com.

.. rubric:: Optional Settings

``pubg:platform``. By default this is set to "steam".

.. rubric:: Usage

The bot will automatically pull your fortnite stats into these variables, with a delay of a few minutes. By default, they will be reset at the end of every stream. You can also use the ``stats:reset`` setting to clear them at a specific hour instead.
