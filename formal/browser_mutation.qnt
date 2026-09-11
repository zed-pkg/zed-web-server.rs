// One configured BFF request. Cryptography, remote authorization and database
// transactions are assumptions, not proof claims. Rust trace replay exercises
// the real async implementation at these same HTTP/cookie boundaries.
module browser_mutation {
  type Phase = Origin | Session | Refresh | Delegate | Api | Done
  type Outcome = Pending | Applied | SignIn | Failed

  type State = {
    same_origin: bool,
    valid_session: bool,
    refresh_result: int, // 0=same principal, 1=rejected, 2=principal changed
    delegation_ok: bool,
    api_result: int, // 0=2xx, 1=rejected, 2=redirect (never success)
    phase: Phase,
    outcome: Outcome,
    refresh_called: bool,
    delegate_called: bool,
    api_called: bool,
    rotated: bool,
    returned_cookie: bool,
    verified_cookie: bool,
  }
  var s: State

  action init = {
    nondet same_origin = Set(false, true).oneOf()
    nondet valid_session = Set(false, true).oneOf()
    nondet refresh_result = Set(0, 1, 2).oneOf()
    nondet delegation_ok = Set(false, true).oneOf()
    nondet api_result = Set(0, 1, 2).oneOf()
    s' = {
      same_origin: same_origin,
      valid_session: valid_session,
      refresh_result: refresh_result,
      delegation_ok: delegation_ok,
      api_result: api_result,
      phase: Origin,
      outcome: Pending,
      refresh_called: false,
      delegate_called: false,
      api_called: false,
      rotated: false,
      returned_cookie: false,
      verified_cookie: false,
    }
  }

  action step = match s.phase {
    | Origin => s' = if (s.same_origin)
        { ...s, phase: Session }
      else { ...s, phase: Done, outcome: Failed }
    | Session => s' = if (s.valid_session)
        { ...s, phase: Refresh }
      else { ...s, phase: Done, outcome: SignIn }
    | Refresh => s' = if (s.refresh_result == 0)
        { ...s, phase: Delegate, refresh_called: true, rotated: true }
      else { ...s, phase: Done, outcome: Failed, refresh_called: true }
    | Delegate => s' = if (s.delegation_ok)
        { ...s, phase: Api, delegate_called: true }
      else {
        ...s, phase: Done, outcome: Failed,
        delegate_called: true, returned_cookie: true,
      }
    | Api => s' = {
        ...s, phase: Done,
        outcome: if (s.api_result == 0) Applied else Failed,
        api_called: true, returned_cookie: true, verified_cookie: true,
      }
    | Done => s' = s
  }

  val refresh_requires_browser_authority = not(s.refresh_called) or and {
    s.same_origin, s.valid_session,
  }
  val delegation_requires_same_principal = not(s.delegate_called) or and {
    s.refresh_called, s.refresh_result == 0, s.rotated,
  }
  val api_requires_scoped_delegation = not(s.api_called) or and {
    s.delegate_called, s.delegation_ok,
    s.same_origin, s.valid_session, s.refresh_result == 0,
  }
  val only_api_success_is_applied =
    (s.outcome == Applied) == and { s.api_called, s.api_result == 0 }
  val redirect_is_not_success = s.api_result != 2 or s.outcome != Applied
  val rotation_is_same_principal =
    s.rotated == and { s.refresh_called, s.refresh_result == 0 }
  val terminal_rotation_is_preserved =
    s.returned_cookie == and { s.phase == Done, s.rotated }
  val verification_requires_successful_delegation =
    s.verified_cookie == and { s.returned_cookie, s.delegate_called, s.delegation_ok }
  val pending_is_not_terminal = (s.outcome == Pending) == (s.phase != Done)
  val sign_in_requires_no_active_session = s.outcome != SignIn or and {
    s.same_origin, not(s.valid_session), not(s.refresh_called),
  }
  val mutation_safety = and {
    refresh_requires_browser_authority,
    delegation_requires_same_principal,
    api_requires_scoped_delegation,
    only_api_success_is_applied,
    redirect_is_not_success,
    rotation_is_same_principal,
    terminal_rotation_is_preserved,
    verification_requires_successful_delegation,
    pending_is_not_terminal,
    sign_in_requires_no_active_session,
  }

  val applied_reached = s.outcome == Applied
  val origin_rejected = s.phase == Done and not(s.same_origin)
  val sign_in_reached = s.outcome == SignIn
  val principal_change_rejected = and {
    s.phase == Done, s.refresh_called, s.refresh_result == 2,
    not(s.delegate_called), not(s.returned_cookie),
  }
  val delegation_failure_keeps_rotation = and {
    s.phase == Done, s.delegate_called, not(s.delegation_ok),
    s.returned_cookie, not(s.api_called),
  }
  val api_failure_keeps_rotation = and {
    s.phase == Done, s.api_called, s.api_result == 1, s.returned_cookie,
  }
  val api_redirect_is_rejected = and {
    s.phase == Done, s.api_called, s.api_result == 2, s.outcome == Failed,
  }
}
