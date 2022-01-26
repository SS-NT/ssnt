use std::hash::Hash;

use crate::tgm::*;
use bevy::utils::HashMap;
use nom::{branch::alt, bytes::complete::{escaped, tag}, character::complete::{multispace0, none_of, one_of}, combinator::{opt, recognize}, error::{ContextError, ParseError, VerboseError, context}, multi::{fold_many1, many1, separated_list0, separated_list1}, number::complete::float, sequence::{delimited, pair, preceded, tuple}};

type IResult<I, O, E = VerboseError<I>> = nom::IResult<I, O, E>;

type MapParseResult<'a> = IResult<&'a str, (Vec<(&'a str, Tile)>, Vec<(UVec3, &'a str)>)>;

pub fn parse(input: &str) -> MapParseResult {
    pair(ws(tile_definitions), ws(chunk_definitions))(input)
}

fn chunk_definitions(input: &str) -> IResult<&str, Vec<(UVec3, &str)>> {
    many1(ws(chunk_definition))(input)
}

fn chunk_definition(input: &str) -> IResult<&str, (UVec3, &str)> {
    context("chunk", assignment(list(number), delimited(tag("{"), string, tag("}"))))(input).map(
        |(input, (floats, name))| {
            (
                input,
                (
                    UVec3::new(
                        // TODO: don't crash if there's not 3 coords -_-
                        *floats.get(0).unwrap() as u32,
                        *floats.get(1).unwrap() as u32,
                        *floats.get(2).unwrap() as u32,
                    ),
                    name,
                ),
            )
        },
    )
}

pub fn tile_definitions(input: &str) -> IResult<&str, Vec<(&str, Tile)>> {
    ws(fold_many1(
        ws(tile_definition),
        Vec::default(),
        |mut map, (key, tile)| {
            // We assume each tile definition is unique
            map.push((key, tile));
            map
        },
    ))(input)
}

fn tile_definition(input: &str) -> IResult<&str, (&str, Tile)> {
    context("tile definition", assignment(string, list(object)))(input)
        .map(|(i, (n, tiles))| (i, (n, tiles.into())))
}

fn path(input: &str) -> IResult<&str, &str> {
    context("path", recognize(preceded(tag("/"), separated_list1(tag("/"), identifier))))(input)
}

fn variable(input: &str) -> IResult<&str, Variable> {
    context("variable", assignment(identifier, value))(input).map(|(i, (ident, val))| (i, Variable::new(ident, val)))
}

fn object(input: &str) -> IResult<&str, Object> {
    context("object",
    pair(
        path,
        opt(delimited(
            tag("{"),
            ws(separated_list0(tag(";"), ws(variable))),
            tag("}"),
        )),
    ))(input)
    .map(|(i, (name, vars))| (i, Object::new(name, vars.unwrap_or_default())))
}

fn string(input: &str) -> IResult<&str, &str> {
    let contents = |sym| escaped(none_of(sym), '\\', tag("\""));
    alt((delimited(tag("\""), contents("\"\\"), tag("\"")), delimited(tag("'"), contents("'\\"), tag("'"))))(input)
}

fn value(input: &str) -> IResult<&str, Value> {
    context("value", alt((
        value_parser(number),
        value_parser(string),
        value_parser(object),
        value_parser(list(value)),
        value_parser(map(alt((string, path)), value)),
        null,
    )))(input)
}

fn value_parser<'a, F: 'a, I: 'a + ?Sized, O, E>(
    mut inner: F,
) -> impl FnMut(&'a I) -> IResult<&'a I, Value, E>
where
    F: FnMut(&'a I) -> IResult<&'a I, O, E>,
    O: Into<Value>,
{
    move |input| inner(input).map(|(i, value)| (i, value.into()))
}

fn null(input: &str) -> IResult<&str, Value> {
    tag("null")(input).map(|(i, _)| (i, (Value::Null)))
}

fn number(input: &str) -> IResult<&str, f64> {
    /*recognize(tuple((opt(tag("-")), digit1, opt(pair(tag("."), digit1)))))(input)
    .map(|(i, val)| (i, val.parse().unwrap()))*/
    context("number", float)(input).map(|(i, n)| (i, n as f64))
}

fn identifier(input: &str) -> IResult<&str, &str> {
    context("identifier",
    recognize(many1(one_of(
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_",
    ))))(input)
}

fn ws<'a, F: 'a, O, E: ParseError<&'a str>>(
    inner: F,
) -> impl FnMut(&'a str) -> IResult<&'a str, O, E>
where
    F: FnMut(&'a str) -> IResult<&'a str, O, E>,
{
    delimited(multispace0, inner, multispace0)
}

/// Parses an assignment in the form of `left = right`, ignoring whitespace.
///
/// ```
/// assert_eq!(assignment(tag("you"), tag("awesome"))("you= awesome"), Ok(("you= awesome", ("you", "awesome"))))
/// ```
fn assignment<'a, FL: 'a, FR: 'a, L, R, E: 'a + ParseError<&'a str> + ContextError<&'a str>>(
    left: FL,
    right: FR,
) -> impl FnMut(&'a str) -> IResult<&'a str, (L, R), E>
where
    FL: FnMut(&'a str) -> IResult<&'a str, L, E>,
    FR: FnMut(&'a str) -> IResult<&'a str, R, E>,
{
    let mut parser = tuple((left, ws(tag("=")), right));
    context("assignment", move |input| parser(input).map(|(i, (l, _, r))| (i, (l, r))))
}

/// Parses a list of another parser
fn list<'a, F: 'a, O, E: 'a + ParseError<&'a str> + ContextError<&'a str>>(
    inner: F,
) -> impl FnMut(&'a str) -> IResult<&'a str, Vec<O>, E>
where
    F: FnMut(&'a str) -> IResult<&'a str, O, E>,
{
    context("list",
    preceded(
        opt(tag("list")),
        delimited(tag("("), separated_list1(tag(","), ws(inner)), tag(")")),
    ))
}

/// Parses a map of another parser
fn map<'a, K, V, KO, VO, E>(
    key: K, value: V
) -> impl FnMut(&'a str) -> IResult<&'a str, HashMap<KO, VO>, E>
where
    K: 'a + FnMut(&'a str) -> IResult<&'a str, KO, E>,
    V: 'a + FnMut(&'a str) -> IResult<&'a str, VO, E>,
    KO: 'a + Eq + Hash,
    VO: 'a, 
    E: 'a + ParseError<&'a str> + ContextError<&'a str>
{
    let mut parser = context("map",
    preceded(
        tag("list"),
        delimited(tag("("), separated_list1(tag(","), ws(assignment(key, value))), tag(")")),
    ));
    move |input| parser(input).map(|(i, o)| (i, o.into_iter().collect()))
}
