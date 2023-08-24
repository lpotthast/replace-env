# replace-env

This crate provides the `ReplaceEnv` trait with it's `replace_env` function.

Types for which this trait is implemented may contain environment variables which the implementation should replace.

The trait is currently implemented for `String` and `Option<T: ReplaceEnv>`.

The `app-properties` crate uses this to transform the configuration it reads.

## MSRV

The minimum supported rust version is `1.64.0`
