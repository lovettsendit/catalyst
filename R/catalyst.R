# Catalyst's own R side (docs/interface.md section 9).
#
# An export written by `catalyst export r` is meant to be handed to somebody
# who does this work in R, and the first thing they should be able to do with
# it is check it. That is what lives here: base R, no package loaded by any
# spelling, so it runs with the R that is already installed.
#
# The rule these helpers enforce is the fixtures' own rule, not a new one. A
# checker that was more forgiving than the declared tolerance would pass
# exports the declared tolerance rejects, and a checker that was stricter
# would fail exports that are correct -- either way the number written in
# `fixtures.R` would stop meaning anything.

# The tolerance rule: within `abs`, or within `rel` of the larger magnitude,
# whichever is more forgiving. The same rule `catalyst::export::close` applies
# on the Rust side and `catalyst_close` applies inside a generated
# `test_function.R`.
#
# The fourth argument is named `abs` because that is what the interface calls
# it, which shadows the base function of that name for value lookups. R
# resolves a name in *call* position to the nearest function of that name, so
# `abs(a - b)` below is still base R's; the `tol_abs` alias is there so a
# reader does not have to know that to trust the line.
catalyst_close <- function(a, b, rel, abs) {
  tol_abs <- abs
  if (is.nan(a) && is.nan(b)) {
    return(TRUE)
  }
  if (!is.finite(a) || !is.finite(b)) {
    return(a == b)
  }
  abs(a - b) <= max(tol_abs, rel * max(abs(a), abs(b)))
}

# Source an export directory into an environment of its own.
#
# Its own, rather than the caller's, because an export defines names as
# ordinary as `catalyst_value`, and checking two exports in one session must
# not let the first one answer for the second.
catalyst_load_export <- function(dir) {
  env <- new.env(parent = globalenv())
  for (file in c("function.R", "parameters.R", "fixtures.R")) {
    path <- file.path(dir, file)
    if (!file.exists(path)) {
      stop(sprintf("the export is not complete: %s is missing", file))
    }
    sys.source(path, envir = env)
  }
  for (name in c("catalyst_value", "catalyst_gradient", "catalyst_in_domain",
                 "catalyst_fixtures", "catalyst_tolerance")) {
    if (!exists(name, envir = env, inherits = FALSE)) {
      stop(sprintf("the export defines no %s", name))
    }
  }
  env
}

# Check every fixture of an export and return how many cases were checked.
#
# It stops, naming the case, on the first disagreement. Returning a count
# rather than TRUE is deliberate: "it passed" and "it checked nothing" look
# identical otherwise, and an export whose fixtures went missing would then
# read as a clean run.
catalyst_check_export <- function(dir) {
  env <- catalyst_load_export(dir)
  fixtures <- get("catalyst_fixtures", envir = env)
  tolerance <- get("catalyst_tolerance", envir = env)
  value_at <- get("catalyst_value", envir = env)
  gradient_at <- get("catalyst_gradient", envir = env)
  in_domain <- get("catalyst_in_domain", envir = env)
  rel <- tolerance$relative
  tol <- tolerance$absolute
  if (length(fixtures) == 0L) {
    stop("the export declares no fixture case, so there is nothing to check")
  }

  checked <- 0L
  for (fixture in fixtures) {
    name <- fixture$name
    inside <- isTRUE(in_domain(fixture$inputs)$ok)
    if (inside == isTRUE(fixture$refused)) {
      stop(sprintf(
        "case %s: the domain check reports inside = %s, and the fixture declares refused = %s",
        name, inside, isTRUE(fixture$refused)))
    }
    if (isTRUE(fixture$nonfinite)) {
      # `nonfinite` is the exporter's word for "the engine had no finite
      # answer here", value or any partial, which is how go/catalyst reads
      # it too; an export that returns every number finite at such a point
      # is not computing the same function.
      got <- value_at(fixture$inputs)
      partials <- unlist(gradient_at(fixture$inputs))
      if (is.finite(got) && all(is.finite(partials))) {
        stop(sprintf(
          "case %s: the fixture declares no finite value or partial here, and the export returned %.17g with every partial finite",
          name, got))
      }
    }
    if (!is.null(fixture$value)) {
      got <- value_at(fixture$inputs)
      if (!catalyst_close(got, fixture$value, rel, tol)) {
        stop(sprintf("case %s: the export computes %.17g where the fixture declares %.17g",
                     name, got, fixture$value))
      }
    }
    if (!is.null(fixture$gradient)) {
      got <- gradient_at(fixture$inputs)
      for (parameter in names(fixture$gradient)) {
        if (!catalyst_close(got[[parameter]], fixture$gradient[[parameter]], rel, tol)) {
          stop(sprintf(
            "case %s: the export computes d/d%s = %.17g where the fixture declares %.17g",
            name, parameter, got[[parameter]], fixture$gradient[[parameter]]))
        }
      }
    }
    checked <- checked + 1L
  }
  checked
}
