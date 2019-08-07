date
----

return a human-readable version of a stored timestamp::

    (date [name])

.. rubric:: Usage

.. code-block:: text

    !phrases set bday 2019-11-04T00:00-0700
    !command add !birthday (channel)'s birthday is on (date bday)
