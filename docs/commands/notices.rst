notices
-------

create a notice to be posted to chat every X seconds::

    !notices add [secs] [command]

.. rubric:: Usage

Notices will be handled intelligently and ones created with the same interval will be rotated through. For example::

    !notices add 300 !command1
    !notices add 300 !command2
    !notices add 600 !command3

will result in one command being posted to chat every five minutes, in the following order::

    !command1
    !command3
    !command2
    !command3
    !command1
    !command3

!command3 is posted every ten minutes, and !command1/!command2 are alternated between.
