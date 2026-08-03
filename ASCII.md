# Forge animation

Whenever Svarog shows an active forge—the view with **Done**, **Skip**, and
**Actual reps**—cycle through the quiet anvil and ten compact spark bursts:

```text
initial → burst 1 → initial → burst 2 → … → initial → burst 10 → repeat
```

Every frame reserves five lines so the controls below it do not move. Every
burst combines Svarog amber (`#FF8C00`) with muted-gray sparks.

`[FORGING IN PROGRESS]` is muted gray in the initial state and amber whenever
sparks are visible.

## Initial state

```text


       ___┬___
          ▔
[FORGING IN PROGRESS]
```

## Anvil spark burst 1

```text
        * * *
      *  * *  *
       ___┬___ *
          ▔
[FORGING IN PROGRESS]
```

## Anvil spark burst 2

```text
       *   * *
     *  *   *  *
     * ___┬___
          ▔
[FORGING IN PROGRESS]
```

## Anvil spark burst 3

```text
         *  *
      *   *   *
     * ___┬___ *
          ▔
[FORGING IN PROGRESS]
```

## Anvil spark burst 4

```text
        *  *  *
     *   *   *
       ___┬___ *
          ▔
[FORGING IN PROGRESS]
```

## Anvil spark burst 5

```text
       *  *  *
      *    *  *
     * ___┬___ *
          ▔
[FORGING IN PROGRESS]
```

## Anvil spark burst 6

```text
         * *  *
     *  *   *  *
       ___┬___  *
          ▔
[FORGING IN PROGRESS]
```

## Anvil spark burst 7

```text
       *    *
      *  * *  *
     * ___┬___ *
          ▔
[FORGING IN PROGRESS]
```

## Anvil spark burst 8

```text
        * *  *
     * *    *  *
       ___┬___
          ▔
[FORGING IN PROGRESS]
```

## Anvil spark burst 9

```text
         *   *
      * *  *  *
       ___┬___ *
          ▔
[FORGING IN PROGRESS]
```

## Anvil spark burst 10

```text
       *  *   *
     *   *  *  *
     * ___┬___
          ▔
[FORGING IN PROGRESS]
```

If animation is unavailable, use the initial state.
