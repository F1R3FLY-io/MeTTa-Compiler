------------------------- MODULE GcDriverChannelProtocol -------------------------
(***************************************************************************)
(* Dedicated GC driver request/response channel protocol.                   *)
(*                                                                         *)
(* StoreRequestSender=FALSE models a driver receive whose request channel   *)
(* has no stored sender. CarryReplySender=FALSE models a Collect request    *)
(* that cannot produce a reply for the caller's response receive.           *)
(* DriverAttemptsReply=FALSE models the driver handling Collect without     *)
(* attempting the paired response send. CallerWaitsAfterSend=FALSE models   *)
(* a response send with no caller waiting on the paired receiver.            *)
(***************************************************************************)

CONSTANTS StoreRequestSender, CarryReplySender, DriverAttemptsReply,
          CallerWaitsAfterSend

VARIABLES
    phase,
    requestSenderStored,
    requestReceiverOwnedByDriver,
    collectSent,
    responseSenderCarried,
    responseReceiverOwnedByCaller,
    driverReceivedCollect,
    replyAttempted,
    callerWaiting,
    callerDone,
    shutdownSent

vars == <<phase, requestSenderStored, requestReceiverOwnedByDriver,
          collectSent, responseSenderCarried, responseReceiverOwnedByCaller,
          driverReceivedCollect, replyAttempted, callerWaiting, callerDone,
          shutdownSent>>

Init ==
    /\ phase = "init"
    /\ requestSenderStored = FALSE
    /\ requestReceiverOwnedByDriver = FALSE
    /\ collectSent = FALSE
    /\ responseSenderCarried = FALSE
    /\ responseReceiverOwnedByCaller = FALSE
    /\ driverReceivedCollect = FALSE
    /\ replyAttempted = FALSE
    /\ callerWaiting = FALSE
    /\ callerDone = FALSE
    /\ shutdownSent = FALSE

SpawnDriver ==
    /\ phase = "init"
    /\ requestSenderStored' = StoreRequestSender
    /\ requestReceiverOwnedByDriver' = TRUE
    /\ phase' = "spawned"
    /\ UNCHANGED <<collectSent, responseSenderCarried,
                  responseReceiverOwnedByCaller, driverReceivedCollect,
                  replyAttempted, callerWaiting, callerDone, shutdownSent>>

SendCollect ==
    /\ phase = "spawned"
    /\ collectSent' = TRUE
    /\ responseSenderCarried' = CarryReplySender
    /\ responseReceiverOwnedByCaller' = TRUE
    /\ callerWaiting' = CallerWaitsAfterSend
    /\ phase' = "sent"
    /\ UNCHANGED <<requestSenderStored, requestReceiverOwnedByDriver,
                  driverReceivedCollect, replyAttempted, callerDone,
                  shutdownSent>>

DriverReceiveCollect ==
    /\ phase = "sent"
    /\ requestReceiverOwnedByDriver
    /\ collectSent
    /\ driverReceivedCollect' = TRUE
    /\ replyAttempted' = DriverAttemptsReply
    /\ callerDone' = (DriverAttemptsReply /\ callerWaiting)
    /\ phase' = "done"
    /\ UNCHANGED <<requestSenderStored, requestReceiverOwnedByDriver,
                  collectSent, responseSenderCarried,
                  responseReceiverOwnedByCaller, callerWaiting, shutdownSent>>

DropShutdown ==
    /\ phase = "spawned"
    /\ shutdownSent' = TRUE
    /\ phase' = "done"
    /\ UNCHANGED <<requestSenderStored, requestReceiverOwnedByDriver,
                  collectSent, responseSenderCarried,
                  responseReceiverOwnedByCaller, driverReceivedCollect,
                  replyAttempted, callerWaiting, callerDone>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ SpawnDriver
    \/ SendCollect
    \/ DriverReceiveCollect
    \/ DropShutdown
    \/ Done

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ StoreRequestSender \in BOOLEAN
    /\ CarryReplySender \in BOOLEAN
    /\ DriverAttemptsReply \in BOOLEAN
    /\ CallerWaitsAfterSend \in BOOLEAN
    /\ phase \in {"init", "spawned", "sent", "done"}
    /\ requestSenderStored \in BOOLEAN
    /\ requestReceiverOwnedByDriver \in BOOLEAN
    /\ collectSent \in BOOLEAN
    /\ responseSenderCarried \in BOOLEAN
    /\ responseReceiverOwnedByCaller \in BOOLEAN
    /\ driverReceivedCollect \in BOOLEAN
    /\ replyAttempted \in BOOLEAN
    /\ callerWaiting \in BOOLEAN
    /\ callerDone \in BOOLEAN
    /\ shutdownSent \in BOOLEAN

RequestReceiveHasProducer ==
    driverReceivedCollect => requestSenderStored /\ requestReceiverOwnedByDriver

ResponseWaitHasProducer ==
    callerWaiting /\ driverReceivedCollect =>
        responseSenderCarried /\ responseReceiverOwnedByCaller /\ replyAttempted

NoOrphanReplySend ==
    replyAttempted =>
        responseSenderCarried /\ responseReceiverOwnedByCaller /\ callerWaiting

FireAndForgetDoesNotWait ==
    shutdownSent => ~callerWaiting /\ ~replyAttempted

=============================================================================
