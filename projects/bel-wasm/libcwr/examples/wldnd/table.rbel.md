# A first WLD&D table

Three programs describe the participants:

- [Mira, the player](player.rbel.md): a person chooses what the character attempts.
- [The dungeon master](dungeon-master.rbel.md): a model runs the adventure.
- [The dice helper](dice-helper.rbel.md): software supplies dice results.

These are ordinary rbel functions returning records. The descriptions and the
sample exchange can be evaluated now. They do not yet start a game, call a model,
roll a random die, wait for a person, or save game state.

The field descriptions such as `integer`, `record`, and `boolean` are proposed
contracts expressed as rbel data. The current resource loader only supports text
fields and local handlers, so these are not installable coworker definitions yet.
There is no new rbel syntax here; the work still needed is to check these richer
contracts and connect them to human input, a model, and software execution.

## Putting the participants together

Load the definitions from all four files together. `wldnd-table` calls the three
participant functions to build the table's description.

```rbel definition=wldnd-table
(def wldnd-table ()
  (join '(id watchtower-table)
    (join (join 'participants
      (join (join (wldnd-player)
        (join (wldnd-dungeon-master)
          (join (wldnd-dice-helper) nil))) nil)) nil)))
(def wldnd-field (key record)
  (if record
    (if (id key (car (car record)))
      (car (cdr (car record)))
      (wldnd-field key (cdr record))) nil))
```

## One exchange

The following program assembles a sample exchange from the other files. The
choices, die face, and narration are handwritten examples. The dice helper
calculates the total and success using the integer library. Nothing in this sample
executes the described requests. `from` and `to` name the participants in the
story; they are not authenticated runtime identities.

```rbel definition=wldnd-example-exchange
(def wldnd-example-exchange ()
  (join
    '((from dungeon-master) (to mira)
      (scene "You stand outside a ruined watchtower. Its wooden door is closed. Something moves upstairs.")
      (question "What do you do?"))
    (join
      (join '(from mira)
        (join '(to dungeon-master) (wldnd-player-answer)))
      (join
        (join '(from dungeon-master)
          (join '(to dice-helper)
            (join '(action roll--check) (wldnd-listening-check))))
        (join
          (join '(from dice-helper)
            (join '(to dungeon-master) (wldnd-example-roll)))
          (join
            '((from dungeon-master) (to mira)
              (narration "You hear two voices arguing upstairs. Neither seems to have heard you.")
              (question "What do you do next?")) nil))))))
```

At execution time the dungeon master would receive a player's attempt, request
a roll when needed, resume with that result, and return narration plus updated
game state. The system would save the turn and show the player their reply.
The person remains free to attempt something we did not anticipate here.

## Running the examples

From the `bel-wasm` directory:

```sh
python3 literate/test.py libcwr/examples/wldnd/table.rbel.md \
  --include libcwr/examples/wldnd/player.rbel.md \
  --include libcwr/examples/wldnd/dungeon-master.rbel.md \
  --include libcwr/examples/wldnd/dice-helper.rbel.md \
  --include literate/integers.rbel.md \
  --rbel target/rbel-command/bin/rbel \
  --report target/wldnd-check.json
```

The checks below exercise the composed table and the exchange. They check
program values, not the behavior of a live game.

```rbel example=wldnd-table-participants
((fn (people)
  (join (wldnd-field 'played-by (car people))
    (join (wldnd-field 'played-by (car (cdr people)))
      (join (wldnd-field 'played-by (car (cdr (cdr people)))) nil))))
 (wldnd-field 'participants (wldnd-table)))
=>
(human model software)
```

```rbel example=wldnd-exchange-dice-request
(wldnd-field 'action (car (cdr (cdr (wldnd-example-exchange)))))
=>
roll--check
```

```rbel example=wldnd-exchange-player-reply
(wldnd-field 'to (car (cdr (cdr (cdr (cdr (wldnd-example-exchange)))))))
=>
mira
```

This example draws on the earlier [WLD&D design record](https://drive.google.com/file/d/1ozzKPOIgNFcyLAIc2-FMD8FdjQLbVbUT/view),
which described a Dungeon Host, players, and a separate dice helper. The
watchtower scene and these programs are new examples written for this workspace.

```rbel example=wldnd-exchange-keeps-roll
((fn (roll)
  (join (wldnd-field 'face roll)
    (join (wldnd-field 'total roll) nil)))
 (car (cdr (cdr (cdr (wldnd-example-exchange))))))
=>
(14 16)
```
