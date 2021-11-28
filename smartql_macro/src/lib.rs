extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::TokenTree;
use quote::quote;
use quote::ToTokens;
use std::str::FromStr;
use syn::Attribute;

fn to_string<T: ToTokens>(token: &T) -> String {
    let mut tokens = proc_macro2::TokenStream::new();
    token.to_tokens(&mut tokens);
    tokens.to_string()
}

fn into_smartql_group_content(attr: &Attribute) -> Option<proc_macro2::TokenStream> {
    if attr.path.segments.first().unwrap().ident.eq("smartql") {
        if let Some(next) = attr.tokens.clone().into_iter().next() {
            if let TokenTree::Group(group) = next {
                return Some(group.stream());
            }
        }
    }
    None
}

fn is_smartql_incremental(attr: &Attribute) -> bool {
    if let Some(content) = into_smartql_group_content(attr) {
        content.into_iter().any(|token| {
            if let TokenTree::Ident(ident) = token {
                ident.eq("incremental")
            } else {
                false
            }
        })
    } else {
        false
    }
}

fn is_smartql_primary(attr: &Attribute) -> bool {
    if let Some(content) = into_smartql_group_content(attr) {
        content.into_iter().any(|token| {
            if let TokenTree::Ident(ident) = token {
                ident.eq("primary")
            } else {
                false
            }
        })
    } else {
        false
    }
}

fn is_smartql_ignore(attr: &Attribute) -> bool {
    if let Some(content) = into_smartql_group_content(attr) {
        content.into_iter().any(|token| {
            if let TokenTree::Ident(ident) = token {
                ident.eq("ignore")
            } else {
                false
            }
        })
    } else {
        false
    }
}

fn gen_accessors(field: &syn::Field, struct_name: &syn::Ident) -> proc_macro2::TokenStream {
    let ident = field.ident.clone().expect("Fields need identifiers!");
    let field_type = &field.ty;

    let getter = format!("get_{}", ident);
    let setter = format!("set_{}", ident);

    let getter = proc_macro2::TokenStream::from_str(getter.as_str()).unwrap();
    let setter = proc_macro2::TokenStream::from_str(setter.as_str()).unwrap();

    let additional_methods = if field.attrs.iter().any(|attr| is_smartql_incremental(attr)) {
        let inc = format!("increment_{}", ident);
        let dec = format!("decrement_{}", ident);

        let inc = proc_macro2::TokenStream::from_str(inc.as_str()).unwrap();
        let dec = proc_macro2::TokenStream::from_str(dec.as_str()).unwrap();

        quote! {
            pub fn #inc(&mut self, value: #field_type) {
                self.#ident = self.#ident + value;
                smartql::internal::coerce_delta_op(&mut self.__field_delta, stringify!(#struct_name), stringify!(#ident), smartql::internal::DeltaOp::Increment);
            }

            pub fn #dec(&mut self, value: #field_type) {
                self.#ident = self.#ident - value;
                smartql::internal::coerce_delta_op(&mut self.__field_delta, stringify!(#struct_name), stringify!(#ident), smartql::internal::DeltaOp::Decrement);
            }
        }
    } else {
        quote! {}
    };

    quote! {
        pub fn #getter(&self) -> &#field_type {
            &self.#ident
        }

        pub fn #setter(&mut self, value: #field_type) {
            smartql::internal::coerce_delta_op(&mut self.__field_delta, stringify!(#struct_name), stringify!(#ident), smartql::internal::DeltaOp::Set);
            self.#ident = value;
        }

        #additional_methods
    }
}

#[proc_macro_derive(SmartQlObject, attributes(smartql))]
pub fn derive_smartql_object(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::ItemStruct);

    let ident = &input.ident;

    let primary_fields = input
        .fields
        .iter()
        .filter(|field| field.attrs.iter().any(|attr| is_smartql_primary(attr)))
        .map(|field| field.clone())
        .collect::<Vec<syn::Field>>();

    if primary_fields.len() == 0 {
        panic!(
            "Need at least one field marked with smartql(primary) for struct `{}`",
            ident
        );
    }

    let mut where_clause = "WHERE ".to_owned();
    let mut first = true;
    for field in primary_fields {
        let identifier = field.ident.expect("Fields need identifiers").to_string();
        if first {
            where_clause.push_str("`");
            where_clause.push_str(identifier.as_str());
            where_clause.push_str("` = ?");
            first = false;
        } else {
            where_clause.push_str(" AND `");
            where_clause.push_str(identifier.as_str());
            where_clause.push_str("` = ?");
        }
    }

    let mut upsert_bindings = "".to_owned();
    let mut upsert_bindings_appends = "".to_owned();
    let mut sql_fields_list = "".to_owned();
    let mut upsert_update_clause = "".to_owned();
    let mut first = true;
    let mut instance_creator_from_row = "".to_owned();
    let mut field_count = 0;
    for field in input.fields.iter() {
        if field.attrs.iter().any(|attr| is_smartql_ignore(attr)) {
            continue;
        }
        field_count = field_count + 1;
        let identifier = field
            .ident
            .clone()
            .expect("Fields need identifiers")
            .to_string();

        if !field.attrs.iter().any(|attr| is_smartql_primary(attr)) {
            upsert_update_clause.push_str(format!("`{}` = ?, ", identifier).as_str());
            upsert_bindings_appends.push_str(format!("self.get_{}(), ", identifier).as_str());
        }
        upsert_bindings.push_str(format!("self.get_{}(), ", identifier).as_str());

        if first {
            sql_fields_list.push_str("`");
            sql_fields_list.push_str(identifier.as_str());
            sql_fields_list.push_str("`");
            first = false;
        } else {
            sql_fields_list.push_str(", `");
            sql_fields_list.push_str(identifier.as_str());
            sql_fields_list.push_str("`");
        }
        instance_creator_from_row.push_str(
            format!(
                r#"{}: row.get("{}"), "#,
                identifier.as_str(),
                identifier.as_str()
            )
            .as_str(),
        );
    }

    let upsert_update_clause = upsert_update_clause[..upsert_update_clause.len() - 2].to_owned();
    let mut upsert_value_placeholders = "?".to_owned();
    for _ in 1..field_count {
        upsert_value_placeholders.push_str(", ?")
    }

    upsert_bindings.push_str(upsert_bindings_appends.as_str());
    let upsert_bindings = proc_macro2::TokenStream::from_str(upsert_bindings.as_str()).unwrap();

    let instance_creator_from_row =
        proc_macro2::TokenStream::from_str(instance_creator_from_row.as_str()).unwrap();

    let table = format!("{}", ident).to_lowercase();

    let select_clause = format!(
        "SELECT {} FROM `{}` {}",
        sql_fields_list.as_str(),
        table.as_str(),
        where_clause.as_str()
    );

    let upsert_all_clause = format!(
        "INSERT INTO `{}` ({}) VALUES ({}) ON DUPLICATE KEY UPDATE {}",
        table.as_str(),
        sql_fields_list,
        upsert_value_placeholders,
        upsert_update_clause
    );

    let upsert_prefix = format!(
        "INSERT INTO `{}` ({}) VALUES ({}) ON DUPLICATE KEY UPDATE ",
        table.as_str(),
        sql_fields_list,
        upsert_value_placeholders
    );

    println!("Select clause: {}", select_clause);
    println!("Upsert all clause: {}", upsert_all_clause);

    let result = quote! {
        use async_trait::async_trait;
        use sqlx::{Database, Row};

        #[async_trait]
        impl SmartQlObject for #ident {
            async fn load(executor: &sqlx::Pool<sqlx::MySql>, args: sqlx::mysql::MySqlArguments) -> sqlx::Result<Option<Self>>
                where Self: Sized {
                let row = sqlx::query_with(#select_clause, args)
                    .fetch_optional(executor)
                    .await?;
                if let Some(row) = row {
                    return Ok(Some(smartql::smartql_init_lazy! {
                        #ident {
                            #instance_creator_from_row
                        }
                    }));
                } else {
                    return Ok(None);
                }
            }

            async fn save_all(&mut self, executor: &sqlx::Pool<sqlx::MySql>) -> sqlx::Result<bool> {
                let result = sqlx::query_with(#upsert_all_clause, smartql::args!([#upsert_bindings]))
                    .execute(executor)
                    .await?;
                self.reset_delta();
                Ok(result.rows_affected() > 0)
            }

            async fn upsert(&mut self, executor: &sqlx::Pool<sqlx::MySql>) -> sqlx::Result<bool> {
                let mut upsert = #upsert_prefix .to_owned();
                let delta = self.get_delta();
                let mut args = sqlx::mysql::MySqlArguments::default();
                for field in Self::fields().into_iter() {
                    self.add_field_to_args(&mut args, field);
                }
                for (field, delta_op) in delta.clone().into_iter() {
                    self.add_field_to_args(&mut args, field);
                    match delta_op {
                        smartql::internal::DeltaOp::Set => upsert.push_str(format!("`{}` = ?, ", field).as_str()),
                        smartql::internal::DeltaOp::Increment => upsert.push_str(format!("`{}` = `{}` + ?, ", field, field).as_str()),
                        smartql::internal::DeltaOp::Decrement => upsert.push_str(format!("`{}` = `{}` - ?, ", field, field).as_str()),
                    }
                }
                let upsert = upsert[..upsert.len() - 2].to_owned();
                println!("Preparing upsert:\n{}\nwith args:\n{:?}", upsert, args);
                let result = sqlx::query_with(upsert.as_str(), args)
                    .execute(executor)
                    .await?;
                self.reset_delta();
                Ok(result.rows_affected() > 0)
            }
        }
    };

    println!("{}", result);

    //return quote! {}.into();
    return result.into();
}

#[proc_macro_attribute]
pub fn smartql_object(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let struct_item = syn::parse_macro_input!(item as syn::ItemStruct);
    let struct_ident = &struct_item.ident;

    let mut accessors = "".to_owned();

    let mut match_pattern = "".to_owned();

    let mut fields_list = "".to_owned();
    let mut fields = "".to_owned();
    for field in struct_item.fields {
        for attr in &field.attrs {
            fields.push_str(to_string(&attr).as_str());
            fields.push_str("\n");
        }
        let field_ident = field
            .ident
            .clone()
            .expect("Fields need identifiers!")
            .to_string();

        if !field.attrs.iter().any(|attr| is_smartql_ignore(attr)) {
            match_pattern.push_str(
                format!(
                    r#""{}" => args.add(self.get_{}()), "#,
                    field_ident, field_ident
                )
                .as_str(),
            );
        }

        fields_list.push_str("\"");
        fields_list.push_str(field_ident.as_str());
        fields_list.push_str("\"");
        fields_list.push_str(",");

        fields.push_str(field_ident.as_str());
        fields.push_str(": ");
        fields.push_str(to_string(&field.ty).as_str());
        fields.push_str(",\n\n");
        accessors.push_str(&gen_accessors(&field, &struct_ident).to_string());
        accessors.push_str("\n");
    }

    let match_pattern = proc_macro2::TokenStream::from_str(match_pattern.as_str()).unwrap();
    let fields_token = proc_macro2::TokenStream::from_str(fields.as_str()).unwrap();
    let fields_list_token = proc_macro2::TokenStream::from_str(fields_list.as_str()).unwrap();
    let accessors_token = proc_macro2::TokenStream::from_str(accessors.as_str()).unwrap();

    let result = quote! {
        #[derive(smartql::SmartQlObject)]
        pub struct #struct_ident {
            #[smartql(ignore)]
            __field_delta: std::collections::HashMap<&'static str, smartql::internal::DeltaOp>,
            #fields_token
        }

        impl #struct_ident {
            #accessors_token
        }

        impl smartql::internal::SmartQlMetaData for #struct_ident {
            fn fields() -> Vec<&'static str> {
                return vec![#fields_list_token]
            }

            fn get_delta(&self) -> &std::collections::HashMap<&'static str, smartql::internal::DeltaOp> {
                &self.__field_delta
            }

            fn reset_delta(&mut self) {
                self.__field_delta = std::collections::HashMap::new();
            }

            fn add_field_to_args(&self, args: &mut sqlx::mysql::MySqlArguments, field: &'static str) {
                use sqlx::Arguments;
                match field {
                    #match_pattern
                    _ => {
                        panic!("Unknown field!");
                    }
                }
            }
        }
    };

    return result.into();
}

#[proc_macro]
pub fn args(item: TokenStream) -> TokenStream {
    let args = syn::parse_macro_input!(item as syn::ExprArray);

    let mut binds = "".to_owned();

    for arg in args.elems {
        binds.push_str(format!("args.add({}); ", arg.to_token_stream()).as_str())
    }

    let binds = proc_macro2::TokenStream::from_str(binds.as_str()).unwrap();

    let result = quote! {
        {
            use sqlx::{Arguments};
            let mut args = sqlx::mysql::MySqlArguments::default();
            #binds
            args
        }
    };

    return result.into();
}

#[proc_macro]
pub fn smartql_init(item: TokenStream) -> TokenStream {
    let mut struct_item = syn::parse_macro_input!(item as syn::ExprStruct);

    let mut field_names = "".to_owned();
    for field in &struct_item.fields {
        if let syn::Member::Named(ident) = &field.member {
            field_names.push_str("(\"");
            field_names.push_str(to_string(ident).as_str());
            field_names.push_str("\", smartql::internal::DeltaOp::Set)");
            field_names.push_str(",")
        }
    }

    let assignment = format!("__field_delta: [{}].iter().cloned().collect()", field_names);
    let token = proc_macro2::TokenStream::from_str(assignment.as_str())
        .unwrap()
        .into();
    struct_item
        .fields
        .insert(0, syn::parse_macro_input!(token as syn::FieldValue));

    return quote! {
        #struct_item
    }
    .into();
}

#[proc_macro]
pub fn smartql_init_lazy(item: TokenStream) -> TokenStream {
    let mut struct_item = syn::parse_macro_input!(item as syn::ExprStruct);

    let assignment = "__field_delta: std::collections::HashMap::new()";
    let token = proc_macro2::TokenStream::from_str(assignment)
        .unwrap()
        .into();
    struct_item
        .fields
        .insert(0, syn::parse_macro_input!(token as syn::FieldValue));

    return quote! {
        #struct_item
    }
    .into();
}
