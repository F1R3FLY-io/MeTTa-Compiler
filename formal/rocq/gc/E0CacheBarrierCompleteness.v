(** E0/cache SATB barrier completeness.

    Source coupling pins each concrete value-dropping mutation site to one of
    the abstract categories below.  The categories cover the E2 value-bearing
    E0/cache surface: rule and module removals, space handles, bytecode and
    tiered caches, eval/match/subgoal/thunk tables, tokenizer/state containers,
    and epoch-protected local caches.  This Rocq file proves the closure
    obligation for that audit: if the source-coupled category shades the removed
    pre-image, or keeps the value in a persistent structural root, sweep cannot
    free a value that a later CESK transition can still touch.
*)

Module MeTTaTron_GC_E0CacheBarrierCompleteness.

Section E0CacheBarrierCompletenessModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Inductive ValueDroppingMutation
      (SymbolBindingOverwrite MutableStateOverwrite NamedSpaceRemove
       TypeVectorRemove TokenRemove TokenClear ActOverlayClear
       SpaceHandleVarRemove ModuleAtomRemove ModuleClear
       RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
       SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
       TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
       TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
       MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
       SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
       ThunkClear ThunkReplace : Addr -> Prop)
      : Addr -> Prop :=
  | removed_symbol_binding :
      forall a, SymbolBindingOverwrite a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_mutable_state :
      forall a, MutableStateOverwrite a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_named_space :
      forall a, NamedSpaceRemove a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_type_vector :
      forall a, TypeVectorRemove a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_token :
      forall a, TokenRemove a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_token :
      forall a, TokenClear a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_act_overlay :
      forall a, ActOverlayClear a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_space_handle_var :
      forall a, SpaceHandleVarRemove a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_module_atom :
      forall a, ModuleAtomRemove a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_module :
      forall a, ModuleClear a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_rule :
      forall a, RuleRemove a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_rules :
      forall a, RuleClear a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | overwritten_space_registry :
      forall a, SpaceRegistryOverwrite a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_space_registry :
      forall a, SpaceRegistryRemove a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_space_registry :
      forall a, SpaceRegistryClear a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | evicted_bytecode_cache :
      forall a, BytecodeCacheEvict a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_bytecode_cache :
      forall a, BytecodeCacheClear a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_tiered_pending_overwrite :
      forall a, TieredPendingOverwrite a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_tiered_pending_cancel :
      forall a, TieredPendingCancel a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_tiered_guard_drop :
      forall a, TieredGuardDrop a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_tiered_pending :
      forall a, TieredClearPending a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_tiered_compiled :
      forall a, TieredClearCompiled a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | evicted_eval_memo :
      forall a, EvalMemoEvict a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_eval_memo :
      forall a, EvalMemoClear a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | evicted_match_cache :
      forall a, MatchCacheEvict a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_match_cache :
      forall a, MatchCacheClear a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_subgoal_stale :
      forall a, SubgoalStale a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_subgoal_overwrite :
      forall a, SubgoalOverwrite a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_subgoal :
      forall a, SubgoalRemove a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_subgoal :
      forall a, SubgoalClear a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_thunk_stale :
      forall a, ThunkStale a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_thunk_overwrite :
      forall a, ThunkOverwrite a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | removed_thunk :
      forall a, ThunkRemove a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | cleared_thunk :
      forall a, ThunkClear a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a
  | replaced_thunk_results :
      forall a, ThunkReplace a ->
        ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
          NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
          ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
          RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
          SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
          TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
          TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
          MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
          SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
          ThunkClear ThunkReplace a.

  Definition RootSet
      (StructuralRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    StructuralRoot a \/ DriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Record E0CacheBarrierCoverage : Prop := {
    value_dropping_mutation_is_shaded :
      forall (SymbolBindingOverwrite MutableStateOverwrite NamedSpaceRemove
              TypeVectorRemove TokenRemove TokenClear ActOverlayClear
              SpaceHandleVarRemove ModuleAtomRemove ModuleClear
              RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
              SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
              TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
              TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
              MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
              SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
              ThunkClear ThunkReplace Shaded : Addr -> Prop),
        (forall a, SymbolBindingOverwrite a -> Shaded a) ->
        (forall a, MutableStateOverwrite a -> Shaded a) ->
        (forall a, NamedSpaceRemove a -> Shaded a) ->
        (forall a, TypeVectorRemove a -> Shaded a) ->
        (forall a, TokenRemove a -> Shaded a) ->
        (forall a, TokenClear a -> Shaded a) ->
        (forall a, ActOverlayClear a -> Shaded a) ->
        (forall a, SpaceHandleVarRemove a -> Shaded a) ->
        (forall a, ModuleAtomRemove a -> Shaded a) ->
        (forall a, ModuleClear a -> Shaded a) ->
        (forall a, RuleRemove a -> Shaded a) ->
        (forall a, RuleClear a -> Shaded a) ->
        (forall a, SpaceRegistryOverwrite a -> Shaded a) ->
        (forall a, SpaceRegistryRemove a -> Shaded a) ->
        (forall a, SpaceRegistryClear a -> Shaded a) ->
        (forall a, BytecodeCacheEvict a -> Shaded a) ->
        (forall a, BytecodeCacheClear a -> Shaded a) ->
        (forall a, TieredPendingOverwrite a -> Shaded a) ->
        (forall a, TieredPendingCancel a -> Shaded a) ->
        (forall a, TieredGuardDrop a -> Shaded a) ->
        (forall a, TieredClearPending a -> Shaded a) ->
        (forall a, TieredClearCompiled a -> Shaded a) ->
        (forall a, EvalMemoEvict a -> Shaded a) ->
        (forall a, EvalMemoClear a -> Shaded a) ->
        (forall a, MatchCacheEvict a -> Shaded a) ->
        (forall a, MatchCacheClear a -> Shaded a) ->
        (forall a, SubgoalStale a -> Shaded a) ->
        (forall a, SubgoalOverwrite a -> Shaded a) ->
        (forall a, SubgoalRemove a -> Shaded a) ->
        (forall a, SubgoalClear a -> Shaded a) ->
        (forall a, ThunkStale a -> Shaded a) ->
        (forall a, ThunkOverwrite a -> Shaded a) ->
        (forall a, ThunkRemove a -> Shaded a) ->
        (forall a, ThunkClear a -> Shaded a) ->
        (forall a, ThunkReplace a -> Shaded a) ->
        forall a,
          ValueDroppingMutation SymbolBindingOverwrite MutableStateOverwrite
            NamedSpaceRemove TypeVectorRemove TokenRemove TokenClear
            ActOverlayClear SpaceHandleVarRemove ModuleAtomRemove ModuleClear
            RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
            SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
            TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
            TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
            MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
            SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
            ThunkClear ThunkReplace a ->
          Shaded a;

    value_dropping_mutation_survives_collection :
      forall (StructuralRoot DriverRoot ShadedDeletion AllocateBlack
              Removed Marked Freed : Addr -> Prop)
             (Edge : Addr -> Addr -> Prop),
        (forall a, Removed a -> ShadedDeletion a) ->
        (forall a, Reach (RootSet StructuralRoot DriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a) ->
        (forall a, Freed a -> ~ Marked a) ->
        forall a, Removed a -> ~ Freed a;

    persistent_rooted_cache_value_survives_collection :
      forall (StructuralRoot DriverRoot ShadedDeletion AllocateBlack
              Registered Marked Freed : Addr -> Prop)
             (Edge : Addr -> Addr -> Prop),
        (forall a, Registered a -> StructuralRoot a) ->
        (forall a, Reach (RootSet StructuralRoot DriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a) ->
        (forall a, Freed a -> ~ Marked a) ->
        forall a, Registered a -> ~ Freed a;

    epoch_validated_lookup_is_current :
      forall (LookupHit Validated CurrentEpoch : Addr -> Prop),
        (forall a, LookupHit a -> Validated a) ->
        (forall a, Validated a -> CurrentEpoch a) ->
        forall a, LookupHit a -> CurrentEpoch a;

    epoch_cleared_cache_has_no_hit :
      forall (CacheCleared LookupHit : Addr -> Prop),
        (forall a, CacheCleared a -> ~ LookupHit a) ->
        forall a, CacheCleared a -> ~ LookupHit a;

    hash_cons_validated_hit_survives :
      forall (HashConsHit Validated Live Marked Freed : Addr -> Prop),
        (forall a, HashConsHit a -> Validated a) ->
        (forall a, Validated a -> Live a) ->
        (forall a, Live a -> Marked a) ->
        (forall a, Freed a -> ~ Marked a) ->
        forall a, HashConsHit a -> ~ Freed a
  }.

  Theorem e0_cache_barrier_coverage :
    E0CacheBarrierCoverage.
  Proof.
    constructor.
    - intros SymbolBindingOverwrite MutableStateOverwrite NamedSpaceRemove
             TypeVectorRemove TokenRemove TokenClear ActOverlayClear
             SpaceHandleVarRemove ModuleAtomRemove ModuleClear
             RuleRemove RuleClear SpaceRegistryOverwrite SpaceRegistryRemove
             SpaceRegistryClear BytecodeCacheEvict BytecodeCacheClear
             TieredPendingOverwrite TieredPendingCancel TieredGuardDrop
             TieredClearPending TieredClearCompiled EvalMemoEvict EvalMemoClear
             MatchCacheEvict MatchCacheClear SubgoalStale SubgoalOverwrite
             SubgoalRemove SubgoalClear ThunkStale ThunkOverwrite ThunkRemove
             ThunkClear ThunkReplace Shaded
             Hsymbol Hstate Hnamed Htype Htoken_remove Htoken_clear Hact
             Hspace_var Hmodule_remove Hmodule_clear Hrule_remove Hrule_clear
             Hspace_overwrite Hspace_remove Hspace_clear Hbytecode_evict
             Hbytecode_clear Htier_overwrite Htier_cancel Htier_guard
             Htier_clear_pending Htier_clear_compiled Heval_evict Heval_clear
             Hmatch_evict Hmatch_clear Hsubgoal_stale Hsubgoal_overwrite
             Hsubgoal_remove Hsubgoal_clear Hthunk_stale Hthunk_overwrite
             Hthunk_remove Hthunk_clear Hthunk_replace a Hremoved.
      destruct Hremoved;
        match goal with
        | H : SymbolBindingOverwrite _ |- _ => apply Hsymbol; exact H
        | H : MutableStateOverwrite _ |- _ => apply Hstate; exact H
        | H : NamedSpaceRemove _ |- _ => apply Hnamed; exact H
        | H : TypeVectorRemove _ |- _ => apply Htype; exact H
        | H : TokenRemove _ |- _ => apply Htoken_remove; exact H
        | H : TokenClear _ |- _ => apply Htoken_clear; exact H
        | H : ActOverlayClear _ |- _ => apply Hact; exact H
        | H : SpaceHandleVarRemove _ |- _ => apply Hspace_var; exact H
        | H : ModuleAtomRemove _ |- _ => apply Hmodule_remove; exact H
        | H : ModuleClear _ |- _ => apply Hmodule_clear; exact H
        | H : RuleRemove _ |- _ => apply Hrule_remove; exact H
        | H : RuleClear _ |- _ => apply Hrule_clear; exact H
        | H : SpaceRegistryOverwrite _ |- _ => apply Hspace_overwrite; exact H
        | H : SpaceRegistryRemove _ |- _ => apply Hspace_remove; exact H
        | H : SpaceRegistryClear _ |- _ => apply Hspace_clear; exact H
        | H : BytecodeCacheEvict _ |- _ => apply Hbytecode_evict; exact H
        | H : BytecodeCacheClear _ |- _ => apply Hbytecode_clear; exact H
        | H : TieredPendingOverwrite _ |- _ => apply Htier_overwrite; exact H
        | H : TieredPendingCancel _ |- _ => apply Htier_cancel; exact H
        | H : TieredGuardDrop _ |- _ => apply Htier_guard; exact H
        | H : TieredClearPending _ |- _ => apply Htier_clear_pending; exact H
        | H : TieredClearCompiled _ |- _ => apply Htier_clear_compiled; exact H
        | H : EvalMemoEvict _ |- _ => apply Heval_evict; exact H
        | H : EvalMemoClear _ |- _ => apply Heval_clear; exact H
        | H : MatchCacheEvict _ |- _ => apply Hmatch_evict; exact H
        | H : MatchCacheClear _ |- _ => apply Hmatch_clear; exact H
        | H : SubgoalStale _ |- _ => apply Hsubgoal_stale; exact H
        | H : SubgoalOverwrite _ |- _ => apply Hsubgoal_overwrite; exact H
        | H : SubgoalRemove _ |- _ => apply Hsubgoal_remove; exact H
        | H : SubgoalClear _ |- _ => apply Hsubgoal_clear; exact H
        | H : ThunkStale _ |- _ => apply Hthunk_stale; exact H
        | H : ThunkOverwrite _ |- _ => apply Hthunk_overwrite; exact H
        | H : ThunkRemove _ |- _ => apply Hthunk_remove; exact H
        | H : ThunkClear _ |- _ => apply Hthunk_clear; exact H
        | H : ThunkReplace _ |- _ => apply Hthunk_replace; exact H
        end.
    - intros StructuralRoot DriverRoot ShadedDeletion AllocateBlack
             Removed Marked Freed Edge Hremoved_shaded Hmark Hsweep a Hremoved
             Hfreed.
      apply (Hsweep a Hfreed).
      apply Hmark.
      apply reach_root.
      right; right; left.
      apply Hremoved_shaded.
      exact Hremoved.
    - intros StructuralRoot DriverRoot ShadedDeletion AllocateBlack
             Registered Marked Freed Edge Hregistered_root Hmark Hsweep a
             Hregistered Hfreed.
      apply (Hsweep a Hfreed).
      apply Hmark.
      apply reach_root.
      left.
      apply Hregistered_root.
      exact Hregistered.
    - intros LookupHit Validated CurrentEpoch Hvalidated Hcurrent a Hhit.
      apply Hcurrent.
      apply Hvalidated.
      exact Hhit.
    - intros CacheCleared LookupHit Hcleared a Hclear.
      apply Hcleared.
      exact Hclear.
    - intros HashConsHit Validated Live Marked Freed Hvalidated Hlive Hmarked
             Hsweep a Hhit Hfreed.
      apply (Hsweep a Hfreed).
      apply Hmarked.
      apply Hlive.
      apply Hvalidated.
      exact Hhit.
  Qed.
End E0CacheBarrierCompletenessModel.

End MeTTaTron_GC_E0CacheBarrierCompleteness.
