-------------------------- MODULE StructChannelPairing --------------------------
(***************************************************************************)
(* Struct-field channel pairing model.                                      *)
(*                                                                         *)
(* StoreSender/StoreReceiver model constructor storage of paired endpoints. *)
(* CloneWorkerReceiver models moving/cloning a receiver into a worker.      *)
(* CloneResponseSender/StoreResponseReceiver model worker response pairing. *)
(* StoreReadySender/ReturnReadyReceiver model one-shot startup signalling.  *)
(***************************************************************************)

CONSTANTS StoreSender, StoreReceiver, CloneWorkerReceiver,
          CloneResponseSender, StoreResponseReceiver,
          StoreReadySender, ReturnReadyReceiver, ConstructDormantWrapper

VARIABLES
    phase,
    pairCreated,
    senderStored,
    receiverStored,
    receiverUsed,
    workerReceiverCloned,
    workerReceive,
    responseSenderCloned,
    responseReceiverStored,
    responseSent,
    readySenderStored,
    readyReceiverReturned,
    readySignalSent,
    readyWait,
    dormantWrapperConstructed,
    dormantWait

vars == <<phase, pairCreated, senderStored, receiverStored, receiverUsed,
          workerReceiverCloned, workerReceive, responseSenderCloned,
          responseReceiverStored, responseSent, readySenderStored,
          readyReceiverReturned, readySignalSent, readyWait,
          dormantWrapperConstructed, dormantWait>>

Init ==
    /\ phase = "init"
    /\ pairCreated = FALSE
    /\ senderStored = FALSE
    /\ receiverStored = FALSE
    /\ receiverUsed = FALSE
    /\ workerReceiverCloned = FALSE
    /\ workerReceive = FALSE
    /\ responseSenderCloned = FALSE
    /\ responseReceiverStored = FALSE
    /\ responseSent = FALSE
    /\ readySenderStored = FALSE
    /\ readyReceiverReturned = FALSE
    /\ readySignalSent = FALSE
    /\ readyWait = FALSE
    /\ dormantWrapperConstructed = FALSE
    /\ dormantWait = FALSE

Construct ==
    /\ phase = "init"
    /\ pairCreated' = TRUE
    /\ senderStored' = StoreSender
    /\ receiverStored' = StoreReceiver
    /\ workerReceiverCloned' = CloneWorkerReceiver
    /\ responseSenderCloned' = CloneResponseSender
    /\ responseReceiverStored' = StoreResponseReceiver
    /\ readySenderStored' = StoreReadySender
    /\ readyReceiverReturned' = ReturnReadyReceiver
    /\ dormantWrapperConstructed' = ConstructDormantWrapper
    /\ phase' = "constructed"
    /\ UNCHANGED <<receiverUsed, workerReceive, responseSent,
                  readySignalSent, readyWait, dormantWait>>

UseEndpoints ==
    /\ phase = "constructed"
    /\ receiverUsed' = TRUE
    /\ workerReceive' = TRUE
    /\ responseSent' = TRUE
    /\ readySignalSent' = StoreReadySender
    /\ readyWait' = ReturnReadyReceiver
    /\ dormantWait' = ConstructDormantWrapper
    /\ phase' = "used"
    /\ UNCHANGED <<pairCreated, senderStored, receiverStored,
                  workerReceiverCloned, responseSenderCloned,
                  responseReceiverStored, readySenderStored,
                  readyReceiverReturned, dormantWrapperConstructed>>

Done ==
    /\ phase = "used"
    /\ UNCHANGED vars

Next ==
    \/ Construct
    \/ UseEndpoints
    \/ Done

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ StoreSender \in BOOLEAN
    /\ StoreReceiver \in BOOLEAN
    /\ CloneWorkerReceiver \in BOOLEAN
    /\ CloneResponseSender \in BOOLEAN
    /\ StoreResponseReceiver \in BOOLEAN
    /\ StoreReadySender \in BOOLEAN
    /\ ReturnReadyReceiver \in BOOLEAN
    /\ ConstructDormantWrapper \in BOOLEAN
    /\ phase \in {"init", "constructed", "used"}
    /\ pairCreated \in BOOLEAN
    /\ senderStored \in BOOLEAN
    /\ receiverStored \in BOOLEAN
    /\ receiverUsed \in BOOLEAN
    /\ workerReceiverCloned \in BOOLEAN
    /\ workerReceive \in BOOLEAN
    /\ responseSenderCloned \in BOOLEAN
    /\ responseReceiverStored \in BOOLEAN
    /\ responseSent \in BOOLEAN
    /\ readySenderStored \in BOOLEAN
    /\ readyReceiverReturned \in BOOLEAN
    /\ readySignalSent \in BOOLEAN
    /\ readyWait \in BOOLEAN
    /\ dormantWrapperConstructed \in BOOLEAN
    /\ dormantWait \in BOOLEAN

StoredReceiveHasProducer ==
    receiverUsed => pairCreated /\ senderStored /\ receiverStored

WorkerReceiveHasProducer ==
    workerReceive => pairCreated /\ senderStored /\ workerReceiverCloned

ResponseSendHasReceiver ==
    responseSent => responseSenderCloned /\ responseReceiverStored

ReadyWaitHasSignal ==
    readyWait => readySenderStored /\ readyReceiverReturned /\ readySignalSent

DormantWrapperNoWait ==
    ~dormantWrapperConstructed => ~dormantWait

=============================================================================
