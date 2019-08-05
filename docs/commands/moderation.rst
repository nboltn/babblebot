moderation
----------

.. rubric:: Links

add a domain to the whitelist::

    !moderation links add [domain]

remove a domain from the whitelist::

    !moderation links remove [domain]

allow subs to post links::

    !moderation links allowsubs

disallow subs from posting links::

    !moderation links blocksubs

.. rubric:: Colors

allow colored messages in chat::

    !moderation colors on

disallow colored messages in chat::

    !moderation colors off

.. rubric:: Caps

limit the number of capital letters in a message::

    !moderation caps [limit] [trigger]
    limit: a percentage of capital letters allowed in a message
    trigger: minimum number of characters in a message required to trigger this filter

turn off caps filter::

    !moderation caps off

.. rubric:: Minimum Account Age

set the minimum account age, in minutes::

    !moderation age set [num]

turn off minimum account age filter::

    !moderation age off

.. rubric:: Display Timeout Reasons

display timeout reasons in chat::

    !moderation display on

do not display timeout reasons in chat::

    !moderation display off
