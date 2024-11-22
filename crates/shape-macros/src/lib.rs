mod attr;

use attr::{Complex, ContainerAttrs, FieldAttrs, VariantAttrs};
use darling::{ast::GenericParamExt, usage::GenericsExt, FromAttributes}; 
use syn::{spanned::Spanned, DeriveInput, LitStr, Variant, WhereClause};
use quote::quote;

#[proc_macro_derive(Shape, attributes(serde, shape))]
pub fn shape(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
  let input = syn::parse_macro_input!(input as syn::DeriveInput);
  match shape_inner(input) {
    Ok(ts) => ts.into(),
    Err(e) => e.write_errors().into(),
  }
}

fn shape_inner(input: DeriveInput) -> Result<proc_macro2::TokenStream, darling::Error> {

  let ident = &input.ident;
  
  let container_attrs = ContainerAttrs::from_attributes(&input.attrs)?;
  
  let mut early = quote!{};

  for ty in [&container_attrs.into, &container_attrs.try_into].into_iter().flatten() {
    early = quote! {
      #early
      if options.is_serialize() {
        return <#ty as ::shape::Shape>::shape(options);
      }
    }
  }

  for ty in [&container_attrs.from, &container_attrs.try_from].into_iter().flatten() {
    early = quote! {
      #early
      if options.is_deserialize() {
        return <#ty as ::shape::Shape>::shape(options);
      }
    }
  }


  let ty;
  match input.data {
    syn::Data::Struct(data) => {
      match data.fields {
        syn::Fields::Unit  => {
          ty = quote!{ ::shape::Type::Null };  
        },
        syn::Fields::Unnamed(fields) => {
          ty = fields_unnamed(&container_attrs, None, &fields)?;
        },
        syn::Fields::Named(fields) => {
          ty = fields_named(&container_attrs, None, &fields)?;
        }
      }
    },
    syn::Data::Enum(data) => {
      let mut variants = vec![];
      for variant in data.variants {

        let variant_attrs = VariantAttrs::from_attributes(&variant.attrs)?;

        if variant_attrs.skip.is_some() {
          continue;
        }
        
        let ident = LitStr::new(&variant.ident.to_string(), variant.ident.span());

        let mut get_name = quote! {
          let mut name = #ident;
        };

        match &container_attrs.rename_all {
          None => {},
          Some(complex) => match complex {
            Complex::Single(rename_all) => {
              let renamed = LitStr::new(&rename_all.apply(&variant.ident.to_string()), variant.ident.span());
              get_name = quote! {
                #get_name;
                name = #renamed;
              }
            },
            Complex::Complex { serialize, deserialize } => {
              if let Some(serialize) = serialize {
                let renamed = LitStr::new(&serialize.apply(&variant.ident.to_string()), variant.ident.span());
                get_name = quote! {
                  #get_name;
                  if options.is_serialize() {
                    name = #renamed;
                  }
                }
              }

              if let Some(deserialize) = deserialize {
                let renamed = LitStr::new(&deserialize.apply(&variant.ident.to_string()), variant.ident.span());
                get_name = quote! {
                  #get_name;
                  if options.is_deserialize() {
                    name = #renamed;
                  }
                }    
              }
            }
          }
        };

        match &variant_attrs.rename {
          None => {},
          Some(complex) => match complex {
            Complex::Single(rename) => {
              let name = LitStr::new(&rename, variant.ident.span());
              get_name = quote! {
                let name = #name;
              }
            },
            Complex::Complex { serialize, deserialize } => {
              if let Some(serialize) = serialize {
                let renamed = LitStr::new(&serialize, variant.ident.span());
                get_name = quote! {
                  #get_name;
                  if options.is_serialize() {
                    name = #renamed;
                  }
                }
              }

              if let Some(deserialize) = deserialize {
                let renamed = LitStr::new(&deserialize, variant.ident.span());
                get_name = quote! {
                  #get_name;
                  if options.is_deserialize() {
                    name = #renamed;
                  }
                }    
              }
            }
          }
        }

        get_name = quote! {
          {
            #get_name;
            name
          }
        };

        let variant_ty = match &variant.fields {
          syn::Fields::Unit => {
            if variant_attrs.untagged.is_some() || container_attrs.untagged.is_some() {
              quote! { ::shape::Type::Null }
            } else {
              match &container_attrs.tag {
                Some(tag) => {
                  let tag = LitStr::new(&tag, variant.span());
                  quote!{ ::shape::Type::Object(
                    ::shape::Object {
                      properties: ::shape::indexmap::IndexMap::from([
                        (
                          String::from(#tag),
                          ::shape::Property {
                            optional: false,
                            readonly: false,
                            ty: ::shape::Type::Literal(::shape::Literal::String(String::from(#get_name)))
                          }
                        )
                      ])
                    })
                  }
                }

                None => {
                  quote! {
                    ::shape::Type::Literal(
                      ::shape::Literal::String(
                        String::from(#get_name)
                      )
                    )
                  }
                }
              }
            }
          }

          syn::Fields::Unnamed(fields) => {
            let fields = fields_unnamed(&container_attrs, Some(&variant_attrs), fields)?;
            join_enum_fields(fields, get_name, &variant, &variant_attrs, &container_attrs)
          }

          syn::Fields::Named(fields) => {
            let fields = fields_named(&container_attrs, Some(&variant_attrs), fields)?;
            join_enum_fields(fields, get_name, &variant, &variant_attrs, &container_attrs)
          }
        };

        let skip_serializing = variant_attrs.skip_serializing.is_some();
        let skip_deserializing = variant_attrs.skip_deserializing.is_some();
      
        variants.push(quote! {
          if options.is_serialize() && !#skip_serializing {
            variants.push(#variant_ty);
          }

          if options.is_deserialize() && !#skip_deserializing {
            variants.push(#variant_ty);  
          }
        });
      };

      ty = quote! {
        let mut variants = vec![];
        #(#variants;)*
        if variants.is_empty() {
          ::shape::Type::Never
        } else {
          ::shape::Type::Or(variants)
        }
      }
    },
    syn::Data::Union(_) => {
      return Err(darling::Error::custom("Unions are not yet supported"))
    }
  }

  let generics = &input.generics;
  
  let (impl_generics, type_generics, where_clause ) = generics.split_for_impl();
  let shape_where =  {
    generics.type_params()
      .filter_map(|param| {
        let ty = param.as_type_param()?;
        Some(
          quote! {
            #ty: ::shape::Shape
          }
        )
      })
      .collect::<Vec<proc_macro2::TokenStream>>()
  };

  let where_clause = match where_clause {
    None => {
      if shape_where.is_empty() {
        quote!{}  
      } else {
        quote! {
          where #(#shape_where),*
        }
      }
    },
    Some(original) => {
      let mut clause = quote! { #original };
      if !shape_where.is_empty() {
        clause = quote! {
          #clause, #(#shape_where),*
        }
      }
      clause
    }
  };

  let implementation = quote! {
    
    impl #impl_generics ::shape::Shape for #ident #type_generics #where_clause {
      fn shape(options: &::shape::ShapeOptions) -> ::shape::Type {
        #early
        #ty
      }
    }
  };

  Ok(implementation)
}

fn fields_unnamed(container_attrs: &ContainerAttrs, _variant_attrs: Option<&VariantAttrs>, fields: &syn::FieldsUnnamed) -> Result<proc_macro2::TokenStream, darling::Error> {

  if fields.unnamed.len() == 1 {
    let inner = fields.unnamed.first().unwrap();
    let ty = &inner.ty;

    let field_attrs = FieldAttrs::from_attributes(&inner.attrs)?;

    let inner = quote!{
      <#ty as ::shape::Shape>::shape(options)
    };

    let field_skip_serializing = field_attrs.skip_serializing.is_some();
    let field_skip_serializing_if = field_attrs.skip_serializing_if.is_some();
    
    let field_skip_deserializing = field_attrs.skip_deserializing.is_some();
    let field_has_default = field_attrs.default.is_some();
    let container_has_default = container_attrs.default.is_some();

    let out = {
      if field_attrs.skip.is_some() {
        quote!{
          ::shape::Type::Null
        }
      } else {
        quote! {
          if options.is_serialize() {
            if #field_skip_serializing {
              ::shape::Type::Null
            } else if #field_skip_serializing_if {
              ::shape::Type::Or(vec![ #inner, ::shape::Type::Undefined ])
            } else {
              #inner
            }
          } else {
            if #field_skip_deserializing {
              ::shape::Type::Or(vec![ ::shape::Type::Undefined, ::shape::Type::Null ])
            } else if #field_has_default || #container_has_default {
              ::shape::Type::Or(vec![ #inner, ::shape::Type::Undefined ])
            } else {
              #inner
            }
          }
        }
      }
    };

    Ok(out)

  } else {

    let mut list = quote!{
      let mut list = vec![];
    };

    for field in fields.unnamed.iter() { 
      let field_attrs = FieldAttrs::from_attributes(&field.attrs)?;
    
      let skip_serializing = field_attrs.skip_serializing.is_some();
      let skip_deserializing = field_attrs.skip_deserializing.is_some();
      let has_default = field_attrs.default.is_some();
      let container_has_default = container_attrs.default.is_some();
      let skip_serializing_if = field_attrs.skip_serializing_if.is_some();

      let ty = &field.ty;
      
      list = quote!{
        #list
        let ty = <#ty as ::shape::Shape>::shape(options);
        if options.is_serialize() {
          if #skip_serializing {
            // do nothing
          } else if #skip_serializing_if {
            list.push(::shape::Type::Or(vec![ty, ::shape::Type::Undefined]));
          } else {
            list.push(ty);
          }
        } else {
          if #skip_deserializing {
            // do nothing
          } else if #has_default || #container_has_default {
            list.push(::shape::Type::Or(vec![ty, ::shape::Type::Undefined]));
          } else {
            list.push(ty);
          }
        }
      }
    }

    let list = quote! {
      {
        #list
        list
      }
    };

    let shape = quote! {
      ::shape::Type::Tuple(::shape::Tuple {
          items: #list,
          rest: None,
        }
      )
    };

    Ok(shape)
  }
}

fn fields_named(container_attrs: &ContainerAttrs, variant_attrs: Option<&VariantAttrs>, fields: &syn::FieldsNamed) -> Result<proc_macro2::TokenStream, darling::Error> {
  
  let shape = if fields.named.is_empty() {
    quote!{
      ::shape::Type::Object(::shape::Object {
        properties: ::shape::indexmap::IndexMap::new(),
      })
    }
  } else {
    let declare_properties = quote! {
      let mut properties = ::shape::indexmap::IndexMap::<String, ::shape::Property>::new();
    };

    let mut populate_properties = quote!{};

    let mut flattened = vec![];

    for field in &fields.named {
      let field_attrs = FieldAttrs::from_attributes(&field.attrs)?;

      if field_attrs.skip.is_some() {
        continue;
      }

      if field_attrs.flatten.is_some() {
        flattened.push(( field.clone(), field_attrs ));
        continue;
      }

      let field_skip_serializing = field_attrs.skip_serializing.is_some();
      let field_skip_serializing_if = field_attrs.skip_serializing_if.is_some();
      
      let field_has_default = field_attrs.default.is_some();
      let container_has_default = container_attrs.default.is_some();
      let field_skip_deserializing = field_attrs.skip_deserializing.is_some();
      
      let optional = quote!{
        if options.is_serialize() {
          if #field_skip_serializing_if {
            true
          } else {
            false
          }
        } else {
          if #field_has_default || #container_has_default {
            true
          } else {
            false
          }
        }
      }; 

      let readonly = false;
      
      let ident = field.ident.clone().unwrap();

      // field name after applying rename, rename_all, and rename_all_fields
      let get_name = {
        let name = ident.to_string();  
        let property_name = LitStr::new(&ident.to_string(), ident.span());  

        let mut get_name = quote!{
          let mut name = #property_name;
        };

        macro_rules! apply_inflection {
          ($complex:expr) => {
            if let Some(complex) = $complex {
              match complex {
                Complex::Single(rename_all) => {
                  let name = LitStr::new(&rename_all.apply(&name), ident.span());
                  get_name = quote!{
                    #get_name
                    name = #name; 
                  }
                },
                Complex::Complex { serialize, deserialize } => {
                  if let Some(serialize) = serialize {
                    let name = LitStr::new(&serialize.apply(&name), ident.span());
                    get_name = quote!{
                      #get_name
                      if options.is_serialize() {
                        name = #name;
                      } 
                    }
                  }

                  if let Some(deserialize) = deserialize {
                    let name = LitStr::new(&deserialize.apply(&name), ident.span());
                    get_name = quote!{
                      #get_name
                      if options.is_deserialize() {
                        name = #name;
                      } 
                    }
                  }
                }
              }
            }
          }
        }

        match variant_attrs {
          None => {
            apply_inflection!(&container_attrs.rename_all);
          },
          Some(variant_attrs) => {
            apply_inflection!(&container_attrs.rename_all_fields);
            apply_inflection!(&variant_attrs.rename_all);
          }
        }

        match field_attrs.rename {
          None => {},
          Some(complex) => {
            match complex {
              Complex::Single(rename) => {
                let name = LitStr::new(&rename, ident.span());
                // override previous get logic
                get_name = quote!{
                  let name = #name;
                }
              },
              Complex::Complex { serialize, deserialize } => {
                if let Some(serialize) = serialize {
                  let name = LitStr::new(&serialize, ident.span());
                  get_name = quote!{
                    #get_name,
                    if options.is_serialize() {
                      name = #name;
                    } 
                  }
                }

                if let Some(deserialize) = deserialize {
                  let name = LitStr::new(&deserialize, ident.span());
                  get_name = quote!{
                    #get_name,
                    if options.is_deserialize() {
                      name = #name;
                    } 
                  }
                }
              }
            }
          }
        }

        get_name = quote!{
          #get_name;
          name
        };

        get_name
      };

      let ty = &field.ty;

      populate_properties = quote! {
        #populate_properties
        
        if (options.is_serialize() && !#field_skip_serializing) || (options.is_deserialize() && !#field_skip_deserializing) {
          properties.insert(
            String::from({ #get_name }), 
            ::shape::Property {
              readonly: #readonly,
              optional: #optional,
              ty: <#ty as ::shape::Shape>::shape(options),
            }
          );
        }        
      }
    }

    let not_flatten = quote! {
      {
        #declare_properties
        #populate_properties
        ::shape::Type::Object(::shape::Object {
          properties,
        })
      }
    };

    #[allow(clippy::let_and_return)]
    let shape = if flattened.is_empty() {
      not_flatten
    } else {
      let mut shape = quote! {
        let mut shape = #not_flatten;
      };

      for (field, field_attrs) in flattened {
        let field_skip_serializing = field_attrs.skip_serializing.is_some();
        let field_skip_serializing_if = field_attrs.skip_serializing_if.is_some();
        
        let field_has_default = field_attrs.default.is_some();
        let container_has_default = container_attrs.default.is_some();
        let field_skip_deserializing = field_attrs.skip_deserializing.is_some();

        let ty = &field.ty;

        shape = quote! {
          #shape
          let flat = <#ty as ::shape::Shape>::shape(options);
          if options.is_serialize() {
            if #field_skip_serializing {
              // do nothing
            } else if #field_skip_serializing_if {
              let optional = ::shape::Type::Or(vec![::shape::Type::Undefined, flat]); 
              shape = ::shape::Type::And(vec![shape, optional]); 
            } else {
              shape = ::shape::Type::And(vec![shape, flat]); 
            }
          } else {
            if #field_skip_deserializing {
              // do nothing
            } else if #field_has_default || #container_has_default {
              let optional = ::shape::Type::Or(vec![::shape::Type::Undefined, flat]); 
              shape = ::shape::Type::And(vec![shape, optional]); 
            } else {
              shape = ::shape::Type::And(vec![shape, flat]); 
            }
          }
        }
      }

      quote!{
        {
          #shape
          shape
        }
      }
    };

    shape
  };

  Ok(shape)
}

fn join_enum_fields(
  fields: proc_macro2::TokenStream,
  get_name: proc_macro2::TokenStream,
  variant: &Variant,
  variant_attrs: &VariantAttrs,
  container_attrs: &ContainerAttrs
) -> proc_macro2::TokenStream {
  if variant_attrs.untagged.is_some() || container_attrs.untagged.is_some() {
    fields
  } else {
    match &container_attrs.tag {
      Some(tag) => {
        let tag = LitStr::new(&tag, variant.span());
        match &container_attrs.content {
          Some(content) => {
            let content = LitStr::new(&content, variant.span());
            quote! {
              ::shape::Type::And(vec![
                ::shape::Type::Object(Object {
                  properties: ::shape::indexmap::IndexMap::from([
                    (
                      String::from(#tag),
                      ::shape::Property {
                        readonly: false,
                        optional: false,
                        ty: ::shape::Type::Literal(::shape::Literal::String(String::from(#get_name)))
                      }
                    )
                  ])
                }),

                ::shape::Type::And(vec![
                  ::shape::Type::Object(Object {
                    properties: ::shape::indexmap::IndexMap::from([
                      (
                        String::from(#content),
                        ::shape::Property {
                          readonly: false,
                          optional: false,
                          ty: ::shape::Type::Object(::shape::Object {
                            properties: ::shape::indexmap::IndexMap::from([
                              (
                                String::from(#content),
                                ::shape::Property {
                                  readonly: false,
                                  optional: false,
                                  ty: #fields,
                                }
                              )
                            ])
                          })
                        }
                      )
                    ])
                  }),
                ])
              ])
            }
          }
          
          None => {
            quote! {
              ::shape::Type::And(vec![
                ::shape::Type::Object(Object {
                  properties: ::shape::indexmap::IndexMap::from([
                    (
                      String::from(#tag),
                      ::shape::Type::Literal(::shape::Literal::String(String::from(#get_name)))
                    )
                  ])
                }),

                #fields,
              ])
            }
          }
        }
      }

      None => {
        quote! {
          ::shape::Type::Object(::shape::Object {
            properties: ::shape::indexmap::IndexMap::from([
              (
                String::from(#get_name),
                ::shape::Property {
                  readonly: false,
                  optional: false,
                  ty: #fields,
                }
              )
            ])
          })
        }
      }
    }
  }
}