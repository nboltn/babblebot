channel
-------

.. rubric:: channel:viewerstats

.. code-block:: text

    !set channel:viewerstats true

Enables the ``(watchtime)`` and ``(watchrank)`` command variables.

.. rubric:: channel:host-message

.. code-block:: text

    !set channel:host-message (name) has just hosted us with (viewers) viewers! They were last playing (game) and you can find them over at (url).

A message to send to chat when you receive a live host. Available variables are: ``(url)``, ``(name)``, ``(game)``, ``(viewers)``.

.. rubric:: channel:autohost-message

.. code-block:: text

    !set channel:autohost-message (name) has just sent us an autohost! They were last playing (game) and you can find them over at (url).

A message to send to chat when you receive an autohost. Available variables are: ``(url)``, ``(name)``, ``(game)``.
